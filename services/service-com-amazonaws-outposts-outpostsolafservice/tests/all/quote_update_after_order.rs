// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateQuote` on a quote an order has ALREADY been submitted
//! from, driven over the wire through the same front door as the rest of this crate's
//! suites (whose harness it reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! > A quote an order was submitted from is this service's record of that order — the
//! > order names an option OF that quote, and the quote's Outpost association is the
//! > store's second record of that order being outstanding on an Outpost — so an
//! > update of such a quote that would RE-DERIVE what the order was submitted under
//! > (the configuration its option is priced on, the country it is quoted for, or its
//! > Outpost association) is refused 400 `ValidationException` and writes nothing,
//! > while an update that re-derives nothing is applied to it like to any other quote.
//!
//! The rows differ along what the rule generalises over: which of the members the
//! quote's derived members follow from the request carries (each one on its own,
//! several at once, and all of them), the value each carries (a different
//! configuration, the list emptied to the modelled `length(min: 0)`, another country),
//! and — for the association, the member the model widens to
//! `OutpostIdentifierOrEmpty` here — every form it admits: another Outpost by bare id,
//! another by ARN, the Outpost the quote already names, and the empty string that
//! removes the association. The members nothing is derived from (the description, the
//! physical constraints) and a request carrying no updatable member at all are the
//! rows the rule leaves alone.
//!
//! What the refused update must not have done is then read off the resources
//! themselves: the quote still offers the option `GetOrder` reports for the order, and
//! the Outpost the order was placed on is still the one the store answers the
//! outstanding order for.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::QUOTE_STATUS_ORDER_SUBMITTED;

use crate::outpost_decommission::decommission;
use crate::quote_lifecycle::{create_quote, get_quote, request as quote_request, QUOTE_ARN_PREFIX};
use crate::quote_options::option_identifier;
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, ABSENT_OUTPOST_ID};
use crate::update_persistence::overlay;

/// The country every site in this suite stands in, which is also the country its
/// quotes are for: the ordering requirement that compares the two has to be met before
/// an order can be submitted from the quote at all.
const COUNTRY: &str = "US";

/// A country that is not [`COUNTRY`], for the row that re-quotes the quote for another
/// one.
const ANOTHER_COUNTRY: &str = "GB";

/// The whole `$.Message` the refusal states, spelled out here rather than read from
/// the crate: the standard validation report around one clause naming the member the
/// quote arrived in and the requirement it failed, and none of the values the request
/// carried.
const REFUSAL: &str = "1 validation error detected: Value at 'quoteIdentifier' failed to satisfy \
                       constraint: the quote must not have an order submitted from it";

/// The `BlockingResourceType` an order still outstanding on an Outpost is reported as,
/// from `smithy_query(query="shape",shape="BlockingResourceType")`'s value list.
const OUTPOST_ORDER_CANCELLABLE: &str = "OUTPOST_ORDER_CANCELLABLE";

/// The `Status` a decommission that nothing holds up answers with, from
/// `smithy_query(query="shape",shape="OutpostDecommissionStatus")`.
const DECOMMISSION_REQUESTED: &str = "REQUESTED";

/// The `Status` a decommission blocked by work still outstanding answers with. See
/// [`DECOMMISSION_REQUESTED`].
const DECOMMISSION_BLOCKED: &str = "BLOCKED";

/// One quote an order was submitted from, and everything that order names.
struct Submitted {
    /// The Outpost the order was placed on, which is also the Outpost its quote is
    /// associated with.
    outpost_id: String,
    /// The quote the order was taken from.
    quote_id: String,
    /// The order that was submitted from it.
    order_id: String,
    /// The `QuoteOptionIdentifier` the order was submitted against — one of the
    /// options that quote offers.
    option_identifier: String,
}

/// The `RequestedCapacities` the quotes of this suite are priced on.
fn capacities(quantity: f64) -> Value {
    json!([{ "QuoteCapacityType": "EC2", "Unit": "c5.24xlarge", "Quantity": quantity }])
}

/// A quote an order HAS been submitted from, built the way a caller builds one: an
/// Outpost an order can be placed on, a quote associated with it, and a `CreateOrder`
/// against the single option that quote offers.
async fn a_quote_an_order_was_submitted_from(outposts: &Outposts, name: &str) -> Submitted {
    let (outpost_id, _) = outposts.site_and_active_outpost(name, COUNTRY).await;
    let quote = create_quote(
        outposts,
        &quote_request(
            COUNTRY,
            capacities(2.0),
            json!({
                "OutpostIdentifier": outpost_id.clone(),
                "RequestedConstraints": [{ "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" }],
                "RequestedPaymentOptions": ["ALL_UPFRONT"],
                "RequestedPaymentTerms": ["THREE_YEARS"],
                "Description": "the quote the order was taken from",
            }),
        ),
    )
    .await;
    let quote_id = text(&quote, "QuoteId");
    let option_identifier = option_identifier(&quote, name);
    let placed = outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id.clone(),
                "PaymentOption": "ALL_UPFRONT",
                "QuoteIdentifier": quote_id.clone(),
                "QuoteOptionIdentifier": option_identifier.clone(),
            })),
        )
        .await;
    let order_id = text(&placed["Order"], "OrderId");

    // The submit is what put the quote in the state this suite is about, so it is
    // asserted rather than assumed: the quote records the order, and names it.
    let read = get_quote(outposts, &quote_id).await;
    assert_eq!(
        text(&read, "QuoteStatus"),
        QUOTE_STATUS_ORDER_SUBMITTED,
        "case {name:?}: the submitted order must have moved the quote, got {read}"
    );
    assert_eq!(
        text(&read, "SubmittedOrderId"),
        order_id,
        "case {name:?}: the quote must name the order submitted from it, got {read}"
    );
    Submitted {
        outpost_id,
        quote_id,
        order_id,
        option_identifier,
    }
}

/// `GetOrder`, returning the `Order` it answered with.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// The `$.Message` a refusal carries.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `quote` offers the option `identifier` names — the check `CreateOrder`
/// resolves a `QuoteOptionIdentifier` by, read off the wire.
fn offers_option(quote: &Value, identifier: &str) -> bool {
    quote
        .get("QuoteOptions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|option| {
            option.get("QuoteOptionIdentifier").and_then(Value::as_str) == Some(identifier)
        })
}

/// The rule's refusing half: every update that would re-derive what the order was
/// submitted under is refused, and writes nothing.
///
/// All rows run against ONE quote on purpose: a refused update changes nothing, so the
/// quote must still be identical to its pre-update record after every one of them.
#[tokio::test]
async fn an_update_that_would_re_derive_what_the_order_was_submitted_under_is_refused() {
    let outposts = Outposts::new();
    let submitted = a_quote_an_order_was_submitted_from(&outposts, "update-after-order").await;
    let (other_id, other_arn) = outposts.site_and_outpost("another-outpost").await;
    let baseline = get_quote(&outposts, &submitted.quote_id).await;
    let quote_arn = format!("{QUOTE_ARN_PREFIX}{}", submitted.quote_id);

    // (case, the identifier the request names the quote by, the patch it carries)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "another configuration to price",
            submitted.quote_id.as_str(),
            json!({ "RequestedCapacities": capacities(4.0) }),
        ),
        (
            "the configuration emptied to the modelled length(min: 0)",
            submitted.quote_id.as_str(),
            json!({ "RequestedCapacities": [] }),
        ),
        (
            "another payment option to price under",
            submitted.quote_id.as_str(),
            json!({ "RequestedPaymentOptions": ["NO_UPFRONT"] }),
        ),
        (
            "the payment options emptied, which prices every modelled one",
            submitted.quote_id.as_str(),
            json!({ "RequestedPaymentOptions": [] }),
        ),
        (
            "another payment term to price under, naming the quote by ARN",
            quote_arn.as_str(),
            json!({ "RequestedPaymentTerms": ["ONE_YEAR"] }),
        ),
        (
            "another country to quote for",
            submitted.quote_id.as_str(),
            json!({ "CountryCode": ANOTHER_COUNTRY }),
        ),
        (
            "the association moved to another Outpost, by bare id",
            submitted.quote_id.as_str(),
            json!({ "OutpostIdentifier": other_id.clone() }),
        ),
        (
            "the association moved to another Outpost, by Outpost ARN",
            submitted.quote_id.as_str(),
            json!({ "OutpostIdentifier": other_arn.clone() }),
        ),
        (
            "the association removed with the empty string",
            submitted.quote_id.as_str(),
            json!({ "OutpostIdentifier": "" }),
        ),
        // The refusal is about what the request asks the service to re-derive, not
        // about whether the value it carries happens to be the stored one.
        (
            "the association named again as the Outpost the quote already names",
            submitted.quote_id.as_str(),
            json!({ "OutpostIdentifier": submitted.outpost_id.clone() }),
        ),
        (
            "a member nothing is derived from, beside one the quote is priced on",
            submitted.quote_id.as_str(),
            json!({
                "Description": "must not land either",
                "RequestedPaymentTerms": ["FIVE_YEARS"],
            }),
        ),
        (
            "every member an update can carry at once",
            quote_arn.as_str(),
            json!({
                "CountryCode": ANOTHER_COUNTRY,
                "Description": "must not land either",
                "OutpostIdentifier": other_id.clone(),
                "RequestedCapacities": capacities(8.0),
                "RequestedConstraints": [],
                "RequestedPaymentOptions": ["PARTIAL_UPFRONT"],
                "RequestedPaymentTerms": ["FIVE_YEARS"],
            }),
        ),
        // The quote's own state is what the refusal is about, and it is resolved
        // before the Outpost the request may also name: on such a quote an
        // `OutpostIdentifier` that names no Outpost is refused for the quote rather
        // than reported absent.
        (
            "the association moved to an Outpost that does not exist",
            submitted.quote_id.as_str(),
            json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID }),
        ),
    ];

    for (case, identifier, patch) in &cases {
        let reply = outposts
            .call(
                "PATCH",
                &format!("/quotes/{}", segment(identifier)),
                Some(patch),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body);
        assert_eq!(
            actual, REFUSAL,
            "case {case:?}: expected $.Message {REFUSAL:?}, got {actual:?}"
        );
        // A refused update wrote nothing: the quote is field for field the record it
        // was before the call, so its option, its association and the description the
        // row sent beside them all stand.
        let read = get_quote(&outposts, &submitted.quote_id).await;
        assert_eq!(
            read, baseline,
            "case {case:?}: a refused update must leave the quote exactly as it was, got {read}"
        );
        // And the reference the order holds still resolves: the option `GetOrder`
        // reports is one the quote offers.
        let order = get_order(&outposts, &submitted.order_id).await;
        let reported = text(&order, "QuoteOptionIdentifier");
        assert_eq!(
            reported, submitted.option_identifier,
            "case {case:?}: expected the order to keep QuoteOptionIdentifier {:?}, got {reported:?}",
            submitted.option_identifier
        );
        assert!(
            offers_option(&read, &reported),
            "case {case:?}: the quote must still offer the option {reported:?} its order was \
             submitted against, got {read}"
        );
    }
}

/// The rule's applying half: an update of the same quote that re-derives nothing is
/// applied to it, and leaves the record of the order alone.
///
/// The rows are applied in sequence to ONE quote, so a row that left a previous row's
/// write behind, or that moved a member it never named, shows up immediately.
#[tokio::test]
async fn an_update_that_re_derives_nothing_is_applied_to_such_a_quote() {
    let outposts = Outposts::new();
    let submitted = a_quote_an_order_was_submitted_from(&outposts, "annotate-after-order").await;
    let quote_arn = format!("{QUOTE_ARN_PREFIX}{}", submitted.quote_id);

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "no updatable member at all",
            submitted.quote_id.as_str(),
            json!({}),
        ),
        (
            "a description, which neither the options nor the checks follow from",
            submitted.quote_id.as_str(),
            json!({ "Description": "re-described after the order" }),
        ),
        (
            "the empty description the modelled length(min: 0) allows, by ARN",
            quote_arn.as_str(),
            json!({ "Description": "" }),
        ),
        (
            "the physical constraints, which nothing is derived from",
            submitted.quote_id.as_str(),
            json!({
                "RequestedConstraints": [
                    { "QuoteConstraintType": "RACK_MAX_WEIGHT_LBS", "Value": "1800" },
                    { "QuoteConstraintType": "RACK_SPACE_CONSTRAINED", "Value": "1" },
                ],
            }),
        ),
        (
            "the constraints emptied to the modelled length(min: 0)",
            submitted.quote_id.as_str(),
            json!({ "RequestedConstraints": [] }),
        ),
    ];

    let mut expected = get_quote(&outposts, &submitted.quote_id).await;
    for (case, identifier, patch) in &cases {
        expected = overlay(&expected, patch);
        let answered = outposts
            .ok(
                "PATCH",
                &format!("/quotes/{}", segment(identifier)),
                Some(patch),
            )
            .await["Quote"]
            .clone();
        assert_eq!(
            answered, expected,
            "case {case:?}: UpdateQuote must answer the updated quote, got {answered}"
        );
        assert_eq!(
            get_quote(&outposts, &submitted.quote_id).await,
            expected,
            "case {case:?}: GetQuote must answer the quote the update reported"
        );
        // The record of the order is untouched by it: the quote still reports the
        // order, and still offers the option that order was submitted against.
        assert!(
            offers_option(&answered, &submitted.option_identifier),
            "case {case:?}: the quote must still offer the option its order was submitted \
             against, got {answered}"
        );
        let reported = text(&answered, "SubmittedOrderId");
        assert_eq!(
            reported, submitted.order_id,
            "case {case:?}: expected the quote to keep SubmittedOrderId {:?}, got {reported:?}",
            submitted.order_id
        );
    }
}

/// The second half of what a refused association move protects: the store's second
/// record of the outstanding order stays on the Outpost the order was PLACED on, and
/// no other Outpost answers for it.
#[tokio::test]
async fn a_refused_association_move_leaves_the_record_of_the_order_where_the_order_is() {
    let outposts = Outposts::new();
    let submitted = a_quote_an_order_was_submitted_from(&outposts, "record-after-order").await;
    let (elsewhere_id, _) = outposts.site_and_outpost_in_service("elsewhere").await;

    let reply = outposts
        .call(
            "PATCH",
            &format!("/quotes/{}", segment(&submitted.quote_id)),
            Some(&json!({ "OutpostIdentifier": elsewhere_id.clone() })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "moving the association of a quote an order was submitted from is refused, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );

    // The Outpost the request tried to move the quote to answers for nothing: its
    // decommission starts, because no order of this store is outstanding on it.
    let elsewhere = decommission(&outposts, &elsewhere_id, &json!({})).await;
    assert_eq!(
        text(&elsewhere, "Status"),
        DECOMMISSION_REQUESTED,
        "no order is outstanding on the Outpost the refused update named, so its decommission \
         starts, got {elsewhere}"
    );
    assert_eq!(
        elsewhere.get("BlockingResourceTypes"),
        None,
        "the Outpost the refused update named must block on nothing, got {elsewhere}"
    );

    // While the Outpost the order really was placed on still answers for it, on both
    // records the store keeps of an order outstanding on an Outpost.
    let placed_on = decommission(&outposts, &submitted.outpost_id, &json!({})).await;
    assert_eq!(
        text(&placed_on, "Status"),
        DECOMMISSION_BLOCKED,
        "the order is still outstanding on the Outpost it was placed on, got {placed_on}"
    );
    assert_eq!(
        placed_on.get("BlockingResourceTypes"),
        Some(&json!([OUTPOST_ORDER_CANCELLABLE])),
        "the order still blocks the decommission of the Outpost it was placed on, got {placed_on}"
    );
    let refused = outposts
        .call(
            "DELETE",
            &format!("/outposts/{}", segment(&submitted.outpost_id)),
            None,
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "deleting the Outpost the order was placed on is refused, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );
    let resource_id = text(&refused.body, "ResourceId");
    assert_eq!(
        resource_id, submitted.order_id,
        "expected the conflict to name order {:?}, got {resource_id:?}",
        submitted.order_id
    );
}
