// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `CreateOrder` resolves about the QUOTE an order is
//! submitted from, driven over the wire through the same front door as the rest of
//! this crate's suites (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Every reference an order names is resolved against the resource that gives it
//!    meaning before anything is persisted, so no order is stored carrying a
//!    reference nothing can resolve: a `QuoteOptionIdentifier` is one of the options
//!    the quote the order is taken from offers (404 `NotFoundException`, the
//!    identifier echo of the value as supplied, for any other), and one named without
//!    the quote it belongs to is a bad request (400 `ValidationException`).
//! 2. A quote's Outpost association is what makes an order submitted from that quote
//!    an order OF an Outpost, so an order is taken from a quote only on the Outpost
//!    that quote is associated with; one placed on any other Outpost is refused (400
//!    `ValidationException`) and leaves the quote, its Outpost and the order store
//!    exactly as they were — so the quote's own Outpost stays deletable, nothing
//!    being outstanding on it.
//! 3. An `OrderingRequirement` is "a requirement that must be met before an order can
//!    be submitted for a quote", so an order is submitted only from a quote with no
//!    requirement unmet; a quote that has one is refused (409 `ConflictException`)
//!    naming the quote in `ResourceId` and exactly the checks it reports `FAIL` for
//!    in the message, and the checks are the ones the store gives at the instant of
//!    the submit.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    ORDERING_REQUIREMENT_FAIL, QUOTE_STATUS_CREATED, QUOTE_STATUS_ORDER_SUBMITTED,
};

use crate::quote_lifecycle::{create_quote, get_quote, request as quote_request};
use crate::quote_options::{
    option_identifier, requirement_status, COUNTRY_CODE_MISMATCH, OPERATING_ADDRESS, OUTPOST_ACTIVE,
    OUTPOST_ID_MISSING, SHIPPING_ADDRESS,
};
use crate::site_and_outpost_lifecycle::{activate_outpost, address, segment, text, Outposts};

/// A `QuoteOptionIdentifier` of the modelled form (`^oqo-[a-f0-9]{17}$`) that this
/// service never minted, so it is an option of no quote it holds: well formed enough
/// for the boundary to pass it through to the handler.
const UNMINTED_QUOTE_OPTION_ID: &str = "oqo-abcdef01234567890";

/// The country every site in this suite stands in, which is also the country the
/// quotes are for — except where a row is about the mismatch.
const COUNTRY: &str = "US";

/// A country that is not [`COUNTRY`], for the row about a quote for another country
/// than the site its Outpost stands on.
const ANOTHER_COUNTRY: &str = "IE";

/// A `CreateOrder` body: the two members the model marks `@required`, with
/// `overrides` merged in.
fn order_request(outpost_identifier: &str, overrides: Value) -> Value {
    let mut body = json!({
        "OutpostIdentifier": outpost_identifier,
        "PaymentOption": "ALL_UPFRONT",
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// What a site records of the two addresses an order is placed against, and whether
/// the Outpost on it has settled in its terminal lifecycle status: the records every
/// ordering check this store can decide is read off.
#[derive(Debug, Clone, Copy)]
struct Setup {
    operating_address: bool,
    shipping_address: bool,
    active: bool,
}

impl Setup {
    /// The setup every check is met on: a site recording both addresses, in
    /// [`COUNTRY`], and an Outpost that has settled.
    const fn met() -> Self {
        Self {
            operating_address: true,
            shipping_address: true,
            active: true,
        }
    }
}

/// One site named `<name>-site` recording the addresses `setup` names, and one
/// Outpost named `name` on it, as that Outpost's `(OutpostId, OutpostArn)`.
///
/// The addresses are the harness's own whole `Address`, in [`COUNTRY`]; an Outpost
/// `setup` wants settled is brought to its terminal lifecycle status the way the
/// harness brings one ([`activate_outpost`]).
async fn an_outpost(outposts: &Outposts, name: &str, setup: Setup) -> (String, String) {
    let mut site = json!({ "Name": format!("{name}-site") });
    if let Some(members) = site.as_object_mut() {
        if setup.operating_address {
            members.insert(
                "OperatingAddress".to_string(),
                address(COUNTRY, "WA", "Seattle"),
            );
        }
        if setup.shipping_address {
            members.insert(
                "ShippingAddress".to_string(),
                address(COUNTRY, "WA", "Kirkland"),
            );
        }
    }
    let site = outposts.create_site(&site).await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    let (outpost_id, outpost_arn) = (text(&outpost, "OutpostId"), text(&outpost, "OutpostArn"));
    if setup.active {
        activate_outpost(outposts, &outpost_id);
    }
    (outpost_id, outpost_arn)
}

/// One quote for `country`, associated with `outpost_identifier` (or with no Outpost
/// at all), as its `QuoteId` and the `QuoteOptionIdentifier` of the single option it
/// offers — the only option an order can be submitted against, since a quote option
/// is minted with the quote it belongs to.
async fn a_quote(
    outposts: &Outposts,
    outpost_identifier: Option<&str>,
    country: &str,
    case: &str,
) -> (String, String) {
    let association = match outpost_identifier {
        Some(identifier) => json!({ "OutpostIdentifier": identifier }),
        None => json!({}),
    };
    let quote = create_quote(
        outposts,
        &quote_request(
            country,
            json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
            association,
        ),
    )
    .await;
    (text(&quote, "QuoteId"), option_identifier(&quote, case))
}

/// Every `OrderId` the store holds, as `ListOrders` reports them — what a refused
/// order must leave exactly as it was.
async fn orders_held(outposts: &Outposts) -> Vec<String> {
    outposts.ok("GET", "/list-orders", None).await["Orders"]
        .as_array()
        .map(|orders| orders.iter().map(|order| text(order, "OrderId")).collect())
        .unwrap_or_default()
}

/// `GetOrder`, returning the `Order` it answered with.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// The error message a refusal carries.
fn message(reply: &Value) -> String {
    text(reply, "Message")
}

/// Assert a refused order left the quote it named exactly as it was, and persisted no
/// order at all.
async fn assert_nothing_persisted(
    outposts: &Outposts,
    quote_id: &str,
    held_before: &[String],
    case: &str,
) {
    let quote = get_quote(outposts, quote_id).await;
    assert_eq!(
        text(&quote, "QuoteStatus"),
        QUOTE_STATUS_CREATED,
        "case {case:?}: a refused order must leave the quote where it was, got {quote}"
    );
    assert!(
        quote.get("SubmittedOrderId").is_none(),
        "case {case:?}: a refused order must not be named on the quote, got {quote}"
    );
    let held = orders_held(outposts).await;
    assert_eq!(
        held, held_before,
        "case {case:?}: a refused order must persist nothing, got {held:?}"
    );
}

/// Which `QuoteOptionIdentifier` a row names, and whether it names the quote that
/// option belongs to.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// The option the row's own quote offers — the one an order can be submitted
    /// against.
    OwnOption,
    /// No quote option at all: the model marks the member optional.
    NoOption,
    /// The option of a DIFFERENT quote of the same Outpost, which is an option this
    /// service minted but not one of this quote's.
    AnotherQuotesOption,
    /// A well-formed option id no quote of this service ever offered.
    UnmintedOption,
    /// The row's own option, sent with no `QuoteIdentifier` to name the quote it
    /// belongs to.
    OptionWithoutItsQuote,
}

/// Rule 1: an order is submitted against an option of the quote it is taken from, and
/// against nothing else.
///
/// The rows span what the rule generalises over: the option the quote itself offers,
/// no option at all, an option of another quote this service minted, an option no
/// quote ever offered, and an option named without its quote.
#[tokio::test]
async fn an_order_is_submitted_only_against_an_option_of_the_quote_it_is_taken_from() {
    let outposts = Outposts::new();
    let (outpost_id, _) = an_outpost(&outposts, "option-outpost", Setup::met()).await;
    let (_, other_quotes_option) =
        a_quote(&outposts, Some(&outpost_id), COUNTRY, "another quote").await;

    let rows: Vec<(&str, Names, u16, &str)> = vec![
        ("the option the quote offers", Names::OwnOption, 200, ""),
        ("no quote option at all", Names::NoOption, 200, ""),
        (
            "an option of another quote of the same Outpost",
            Names::AnotherQuotesOption,
            404,
            "NotFoundException",
        ),
        (
            "an option no quote ever offered",
            Names::UnmintedOption,
            404,
            "NotFoundException",
        ),
        (
            "an option named without the quote it belongs to",
            Names::OptionWithoutItsQuote,
            400,
            "ValidationException",
        ),
    ];

    for (case, names, status, code) in &rows {
        // One quote per row: a quote takes exactly one order.
        let (quote_id, own_option) = a_quote(&outposts, Some(&outpost_id), COUNTRY, case).await;
        let (quote, option) = match names {
            Names::OwnOption => (Some(quote_id.clone()), Some(own_option.clone())),
            Names::NoOption => (Some(quote_id.clone()), None),
            Names::AnotherQuotesOption => {
                (Some(quote_id.clone()), Some(other_quotes_option.clone()))
            }
            Names::UnmintedOption => (
                Some(quote_id.clone()),
                Some(UNMINTED_QUOTE_OPTION_ID.to_string()),
            ),
            Names::OptionWithoutItsQuote => (None, Some(own_option.clone())),
        };
        let mut overrides = json!({});
        if let Some(members) = overrides.as_object_mut() {
            if let Some(quote) = &quote {
                members.insert("QuoteIdentifier".to_string(), json!(quote));
            }
            if let Some(option) = &option {
                members.insert("QuoteOptionIdentifier".to_string(), json!(option));
            }
            if quote.is_none() {
                // An order has to name either the line items that make it up or the
                // quote to take them from, so a row that names no quote names an item.
                members.insert(
                    "LineItems".to_string(),
                    json!([{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }]),
                );
            }
        }
        let held_before = orders_held(&outposts).await;
        let reply = outposts
            .call("POST", "/orders", Some(&order_request(&outpost_id, overrides)))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        match status {
            200 => {
                let order = reply.body["Order"].clone();
                assert_eq!(
                    order.get("QuoteOptionIdentifier").and_then(Value::as_str),
                    option.as_deref(),
                    "case {case:?}: expected the quote option the request named, got {order}"
                );
                // Read back through the implemented reader: what was answered is what
                // was persisted.
                let order_id = text(&order, "OrderId");
                let read = get_order(&outposts, &order_id).await;
                assert_eq!(
                    read.get("QuoteOptionIdentifier"),
                    order.get("QuoteOptionIdentifier"),
                    "case {case:?}: GetOrder must report the stored quote option, got {read}"
                );
                assert_eq!(
                    text(&read, "QuoteIdentifier"),
                    quote_id,
                    "case {case:?}: the order must name the quote it was taken from, got {read}"
                );
                let quote = get_quote(&outposts, &quote_id).await;
                assert_eq!(
                    text(&quote, "SubmittedOrderId"),
                    order_id,
                    "case {case:?}: the quote must name the order submitted from it, got {quote}"
                );
            }
            404 => {
                // The identifier echo: the member the reference arrived in, and the
                // value exactly as supplied.
                let expected = format!(
                    "quoteOptionIdentifier={}",
                    option.clone().unwrap_or_default()
                );
                assert_eq!(
                    message(&reply.body),
                    expected,
                    "case {case:?}: expected $.Message {expected:?}, got {}",
                    reply.body
                );
                assert_nothing_persisted(&outposts, &quote_id, &held_before, case).await;
            }
            _ => {
                let rendered = message(&reply.body);
                assert!(
                    rendered.contains("validation error")
                        && rendered.contains("quoteOptionIdentifier"),
                    "case {case:?}: the refusal must state the member and its requirement, \
                     got {rendered:?}"
                );
                assert_nothing_persisted(&outposts, &quote_id, &held_before, case).await;
            }
        }
    }
}

/// Rule 2: an order taken from a quote is placed on the Outpost that quote is
/// associated with.
///
/// The rows span what the rule generalises over: the quote's own Outpost named by
/// each of the two identifier forms the modelled `OutpostIdentifier` accepts, and
/// another Outpost of the same region named by each of them.
#[tokio::test]
async fn an_order_from_a_quote_is_placed_on_the_outpost_the_quote_is_associated_with() {
    let outposts = Outposts::new();
    let (quoted_id, quoted_arn) = an_outpost(&outposts, "quoted-outpost", Setup::met()).await;
    let (other_id, other_arn) = an_outpost(&outposts, "other-outpost", Setup::met()).await;

    let rows: Vec<(&str, String, u16)> = vec![
        (
            "the quote's own Outpost by bare id",
            quoted_id.clone(),
            200,
        ),
        ("the quote's own Outpost by ARN", quoted_arn.clone(), 200),
        ("another Outpost by bare id", other_id.clone(), 400),
        ("another Outpost by ARN", other_arn.clone(), 400),
    ];

    for (case, outpost_identifier, status) in &rows {
        // One quote per row: a quote takes exactly one order.
        let (quote_id, option) = a_quote(&outposts, Some(&quoted_id), COUNTRY, case).await;
        let held_before = orders_held(&outposts).await;
        let body = order_request(
            outpost_identifier,
            json!({
                "QuoteIdentifier": quote_id.clone(),
                "QuoteOptionIdentifier": option.clone(),
            }),
        );
        let reply = outposts.call("POST", "/orders", Some(&body)).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        match status {
            200 => {
                let order = reply.body["Order"].clone();
                // Whichever form named it, the order is placed on that one Outpost.
                assert_eq!(
                    text(&order, "OutpostId"),
                    quoted_id,
                    "case {case:?}: expected the order on {quoted_id}, got {order}"
                );
                let quote = get_quote(&outposts, &quote_id).await;
                assert_eq!(
                    text(&quote, "QuoteStatus"),
                    QUOTE_STATUS_ORDER_SUBMITTED,
                    "case {case:?}: submitting an order moves the quote, got {quote}"
                );
            }
            _ => {
                assert_eq!(
                    reply.error_code, "ValidationException",
                    "case {case:?}: expected a ValidationException, got {} {}",
                    reply.error_code, reply.body
                );
                let rendered = message(&reply.body);
                assert!(
                    rendered.contains("validation error") && rendered.contains("quoteIdentifier"),
                    "case {case:?}: the refusal must state the member and its requirement, \
                     got {rendered:?}"
                );
                assert_nothing_persisted(&outposts, &quote_id, &held_before, case).await;
                // And the Outpost the order was refused on holds nothing either.
                let other = outposts.get_outpost(&other_id).await;
                assert_eq!(
                    text(&other, "OutpostId"),
                    other_id,
                    "case {case:?}: the Outpost the order named must still be readable, \
                     got {other}"
                );
            }
        }
    }
}

/// Rule 2's second half: because the refused order was never placed, nothing is
/// outstanding on the Outpost the quote names, so that Outpost is still deletable.
///
/// A store of its own: what a delete is refused for is what the store holds, so the
/// case starts from one holding only the two Outposts and the quote.
#[tokio::test]
async fn an_order_refused_on_another_outpost_leaves_the_quotes_outpost_deletable() {
    let case = "an order of the quote of one Outpost, placed on another";
    let outposts = Outposts::new();
    let (quoted_id, _) = an_outpost(&outposts, "deletable-outpost", Setup::met()).await;
    let (other_id, _) = an_outpost(&outposts, "ordered-elsewhere", Setup::met()).await;
    let (quote_id, option) = a_quote(&outposts, Some(&quoted_id), COUNTRY, case).await;

    let refused = outposts
        .call(
            "POST",
            "/orders",
            Some(&order_request(
                &other_id,
                json!({
                    "QuoteIdentifier": quote_id.clone(),
                    "QuoteOptionIdentifier": option,
                }),
            )),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );

    let deleted = outposts
        .call("DELETE", &format!("/outposts/{quoted_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: no order was placed on {quoted_id}, so nothing is outstanding on it \
         and it is deletable, got {} {}",
        deleted.status, deleted.body
    );
    let gone = outposts
        .call("GET", &format!("/outposts/{quoted_id}"), None)
        .await;
    assert_eq!(
        gone.status, 404,
        "case {case:?}: the deleted Outpost is gone to every later call, got {} {}",
        gone.status, gone.body
    );
}

/// Rule 3: an order is submitted only from a quote with no ordering requirement
/// unmet, and the refusal names exactly the checks the quote reports `FAIL` for.
///
/// The rows span every check this store can decide: the quote's Outpost association,
/// the Outpost's lifecycle status, each of the site's two addresses, the country the
/// quote is for against the country of that site, several unmet at once, and the
/// setup on which every check is met.
#[tokio::test]
async fn an_order_is_submitted_only_from_a_quote_whose_requirements_are_met() {
    let outposts = Outposts::new();
    // (case, the records the checks are read off, whether the quote names the Outpost,
    //  the country the quote is for, the checks expected unmet)
    let rows: Vec<(&str, Setup, bool, &str, &[&str])> = vec![
        ("every check met", Setup::met(), true, COUNTRY, &[]),
        (
            "a quote associated with no Outpost",
            Setup::met(),
            false,
            COUNTRY,
            &[OUTPOST_ID_MISSING],
        ),
        (
            "an Outpost that has not settled in its active status",
            Setup {
                active: false,
                ..Setup::met()
            },
            true,
            COUNTRY,
            &[OUTPOST_ACTIVE],
        ),
        (
            "a site with no operating address",
            Setup {
                operating_address: false,
                ..Setup::met()
            },
            true,
            COUNTRY,
            &[OPERATING_ADDRESS],
        ),
        (
            "a site with no shipping address",
            Setup {
                shipping_address: false,
                ..Setup::met()
            },
            true,
            COUNTRY,
            &[SHIPPING_ADDRESS],
        ),
        (
            "a quote for another country than the site",
            Setup::met(),
            true,
            ANOTHER_COUNTRY,
            &[COUNTRY_CODE_MISMATCH],
        ),
        (
            "a pending Outpost on a site with neither address",
            Setup {
                operating_address: false,
                shipping_address: false,
                active: false,
            },
            true,
            COUNTRY,
            &[OUTPOST_ACTIVE, OPERATING_ADDRESS, SHIPPING_ADDRESS],
        ),
    ];

    for (index, (case, setup, associated, country, unmet)) in rows.iter().enumerate() {
        // An Outpost of its own per row: each row's checks are read off its own site
        // and its own Outpost.
        let (outpost_id, _) = an_outpost(&outposts, &format!("requirements-{index}"), *setup).await;
        let association = associated.then(|| outpost_id.clone());
        let (quote_id, option) = a_quote(&outposts, association.as_deref(), country, case).await;

        // The quote itself reports exactly those checks as failed, which is what the
        // refusal below has to name.
        let quote = get_quote(&outposts, &quote_id).await;
        let failed: Vec<&str> = [
            OUTPOST_ID_MISSING,
            OUTPOST_ACTIVE,
            OPERATING_ADDRESS,
            SHIPPING_ADDRESS,
            COUNTRY_CODE_MISMATCH,
        ]
        .into_iter()
        .filter(|check| {
            requirement_status(&quote, check).as_deref() == Some(ORDERING_REQUIREMENT_FAIL)
        })
        .collect();
        assert_eq!(
            failed, *unmet,
            "case {case:?}: expected the quote to report {unmet:?} failed, got {quote}"
        );

        let held_before = orders_held(&outposts).await;
        let body = order_request(
            &outpost_id,
            json!({
                "QuoteIdentifier": quote_id.clone(),
                "QuoteOptionIdentifier": option,
            }),
        );
        let reply = outposts.call("POST", "/orders", Some(&body)).await;
        if unmet.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: every requirement is met, so the order is submitted, got {} {}",
                reply.status, reply.body
            );
            let order_id = text(&reply.body["Order"], "OrderId");
            let quote = get_quote(&outposts, &quote_id).await;
            assert_eq!(
                (
                    text(&quote, "QuoteStatus"),
                    text(&quote, "SubmittedOrderId")
                ),
                (QUOTE_STATUS_ORDER_SUBMITTED.to_string(), order_id),
                "case {case:?}: submitting an order moves the quote and names it, got {quote}"
            );
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceId"),
            quote_id,
            "case {case:?}: the conflict is about the quote, got {}",
            reply.body
        );
        let rendered = message(&reply.body);
        for check in *unmet {
            assert!(
                rendered.contains(check),
                "case {case:?}: the refusal must name the unmet {check}, got {rendered:?}"
            );
        }
        for check in [
            OUTPOST_ID_MISSING,
            OUTPOST_ACTIVE,
            OPERATING_ADDRESS,
            SHIPPING_ADDRESS,
            COUNTRY_CODE_MISMATCH,
        ]
        .into_iter()
        .filter(|check| !unmet.contains(check))
        {
            assert!(
                !rendered.contains(check),
                "case {case:?}: {check} is met, so the refusal must not name it, got {rendered:?}"
            );
        }
        assert_nothing_persisted(&outposts, &quote_id, &held_before, case).await;
    }
}

/// Rule 3's last clause: the checks are run at the instant of the submit, so a quote
/// priced while a check was unmet takes an order once the store meets it — and one
/// priced while every check was met is refused once the store stops meeting it.
#[tokio::test]
async fn the_requirements_are_checked_against_the_store_at_the_instant_of_the_submit() {
    let case = "a quote priced while its Outpost had not settled";
    let outposts = Outposts::new();
    let (outpost_id, _) = an_outpost(
        &outposts,
        "settles-later",
        Setup {
            active: false,
            ..Setup::met()
        },
    )
    .await;
    let (quote_id, option) = a_quote(&outposts, Some(&outpost_id), COUNTRY, case).await;
    let body = order_request(
        &outpost_id,
        json!({
            "QuoteIdentifier": quote_id.clone(),
            "QuoteOptionIdentifier": option,
        }),
    );

    let refused = outposts.call("POST", "/orders", Some(&body)).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: the Outpost has not settled, so the order is refused, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );

    // The one record the check reads changes, and nothing else does.
    activate_outpost(&outposts, &outpost_id);
    let served = outposts.call("POST", "/orders", Some(&body)).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: the check is met now, so the same order is submitted, got {} {}",
        served.status, served.body
    );
    let quote = get_quote(&outposts, &quote_id).await;
    assert_eq!(
        text(&quote, "SubmittedOrderId"),
        text(&served.body["Order"], "OrderId"),
        "case {case:?}: the quote must name the order submitted from it, got {quote}"
    );
    // The order really is on the Outpost whose records the check was read off.
    assert_eq!(
        text(&served.body["Order"], "OutpostId"),
        outpost_id,
        "case {case:?}: expected the order on {outpost_id}, got {}",
        served.body
    );
}
