// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteQuote`, driven over the wire through the same front
//! door as the rest of this crate's suites (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A delete removes the quote the identifier names from the region's inventory and
//!    answers an output with no members: the quote is observably gone to every later
//!    call (a read of it is not-found, a list no longer reports it), while every other
//!    quote, and every other resource, is exactly as it was. Both forms the modelled
//!    `QuoteIdentifier` pattern accepts name the same stored record, and the quote's
//!    own status is not a condition of the delete — this operation models no conflict
//!    to refuse one with.
//! 2. An identifier this region's inventory holds no quote for is 404
//!    `NotFoundException`, with the identifier echo of the value AS SUPPLIED; one that
//!    is no quote identifier at all is 400 `ValidationException`. A refused delete
//!    removes nothing.
//! 3. A quote belongs to the region it was created in, so another region's endpoint
//!    cannot delete it: there the identifier names nothing, and the quote stays
//!    readable in its own region.

use std::collections::BTreeSet;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    QUOTE_STATUS_CREATED, QUOTE_STATUS_EXPIRED, QUOTE_STATUS_ORDER_SUBMITTED,
};

use crate::quote_lifecycle::{
    create_quote, request as quote_request, ABSENT_QUOTE_ID, QUOTE_ARN_PREFIX,
};
use crate::site_and_outpost_lifecycle::{
    refusal, segment, text, Outposts, Refusal, ABSENT_OUTPOST_ID, HOME_REGION,
};

/// A region other than [`HOME_REGION`], for the rule that a quote is reachable only
/// through the region it was created in.
const OTHER_REGION: &str = "eu-west-1";

/// How a row puts its quote in the status it is deleted from.
///
/// The three values the modelled `QuoteStatus` declares are reached in three
/// different ways, and the delete is not conditional on any of them, so each is a row:
/// `CREATED` is what a create answers, `EXPIRED` is the clock passing the quote's own
/// `ExpirationDate`, and `ORDER_SUBMITTED` is an order really being submitted from the
/// quote through `CreateOrder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Created,
    Expired,
    OrderSubmitted,
}

impl Status {
    /// The modelled `QuoteStatus` value a quote in this state reports.
    fn modelled(self) -> &'static str {
        match self {
            Self::Created => QUOTE_STATUS_CREATED,
            Self::Expired => QUOTE_STATUS_EXPIRED,
            Self::OrderSubmitted => QUOTE_STATUS_ORDER_SUBMITTED,
        }
    }
}

/// Which of the two identifier forms the modelled `QuoteIdentifier` pattern accepts a
/// row names its quote by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// The bare `oq-…` id.
    BareId,
    /// The full `arn:…:quote/oq-…` ARN.
    Arn,
}

impl Form {
    /// `quote_id` written in this form.
    fn of(self, quote_id: &str) -> String {
        match self {
            Self::BareId => quote_id.to_string(),
            Self::Arn => format!("{QUOTE_ARN_PREFIX}{quote_id}"),
        }
    }
}

/// `DeleteQuote` on an identifier, returning the whole reply.
async fn delete_quote(
    outposts: &Outposts,
    region: &str,
    identifier: &str,
) -> crate::site_and_outpost_lifecycle::Reply {
    outposts
        .call_in(
            region,
            "DELETE",
            &format!("/quotes/{}", segment(identifier)),
            None,
        )
        .await
}

/// Whether a reply carried a payload with no members — what a client sees for an
/// output shape that declares none, whether it arrives as `{}` or as no body at all.
fn is_empty_payload(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// The `message` a refusal carried, under either casing the protocols render it in.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `QuoteId` of every quote `ListQuotes` reports in `region`.
async fn listed_ids(outposts: &Outposts, region: &str) -> BTreeSet<String> {
    let output = outposts.ok_in(region, "GET", "/quotes", None).await;
    output
        .get("Quotes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListQuotes answers a Quotes list, got {output}"))
        .iter()
        .map(|summary| text(summary, "QuoteId"))
        .collect()
}

/// Age the stored quote so the expiry the clock owes it has already fallen due — the
/// seam [`crate::quote_lifecycle`] uses, because a quote's validity window is far
/// longer than a test may sleep for.
fn expire(outposts: &Outposts, quote_id: &str) {
    let aged = DateTime::from_secs_f64(
        DateTime::from(std::time::SystemTime::now()).as_secs_f64() - 60.0,
    );
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .quotes
        .get_mut(quote_id)
        .unwrap_or_else(|| panic!("the create persisted quote {quote_id}"));
    record.quote.expiration_date = Some(aged);
}

/// Rule 1: the delete removes the quote whatever status it holds and whichever
/// identifier form named it, leaves every other quote alone, and answers an output
/// with no members.
///
/// The rows differ along everything the rule generalises over: how much the quote
/// carries (nothing optional, several capacities of every modelled type, constraints,
/// payment options and terms, a description), whether it is associated with an
/// Outpost, the status it is deleted from, and the identifier form the delete names it
/// by.
#[tokio::test]
async fn deleting_a_quote_removes_it_whatever_its_status_and_identifier_form() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("quote-delete").await;

    let cases: Vec<(&str, Value, Status, Form)> = vec![
        (
            "required members only, deleted by bare id",
            quote_request("US", json!([]), json!({})),
            Status::Created,
            Form::BareId,
        ),
        (
            "one EC2 capacity, deleted by ARN",
            quote_request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
                json!({}),
            ),
            Status::Created,
            Form::Arn,
        ),
        (
            "every modelled capacity type and a description",
            quote_request(
                "GB",
                json!([
                    { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 12.0 },
                    { "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 2.5 },
                    { "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 48.0 },
                ]),
                json!({ "Description": "three capacities" }),
            ),
            Status::Created,
            Form::BareId,
        ),
        (
            "constraints, every payment option and term, deleted by ARN",
            quote_request(
                "DE",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                json!({
                    "RequestedConstraints": [{ "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" }],
                    "RequestedPaymentOptions": ["ALL_UPFRONT", "NO_UPFRONT", "PARTIAL_UPFRONT"],
                    "RequestedPaymentTerms": ["ONE_YEAR", "THREE_YEARS", "FIVE_YEARS"],
                }),
            ),
            Status::Created,
            Form::Arn,
        ),
        (
            "associated with an Outpost by bare id, past its expiry",
            quote_request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 8.0 }]),
                json!({ "OutpostIdentifier": outpost_id.clone() }),
            ),
            Status::Expired,
            Form::BareId,
        ),
        (
            "associated with an Outpost by ARN, past its expiry, deleted by ARN",
            quote_request("FR", json!([]), json!({ "OutpostIdentifier": outpost_arn.clone() })),
            Status::Expired,
            Form::Arn,
        ),
        (
            "an order already submitted from it",
            quote_request(
                "US",
                json!([{ "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 100.0 }]),
                json!({ "OutpostIdentifier": outpost_id.clone() }),
            ),
            Status::OrderSubmitted,
            Form::BareId,
        ),
        (
            "an order already submitted from it, deleted by ARN",
            quote_request(
                "CA",
                json!([{ "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 0.5 }]),
                json!({ "OutpostIdentifier": outpost_id.clone() }),
            ),
            Status::OrderSubmitted,
            Form::Arn,
        ),
    ];

    for (case, body, status, form) in &cases {
        // A bystander quote per row: deleting one quote must leave every other one
        // exactly as it was, which only a second record in the store can show.
        let bystander = create_quote(
            &outposts,
            &quote_request("US", json!([]), json!({ "Description": "bystander" })),
        )
        .await;
        let bystander_id = text(&bystander, "QuoteId");

        let created = create_quote(&outposts, body).await;
        let quote_id = text(&created, "QuoteId");

        // Put the quote in the status this row deletes it from, each by the thing
        // that really reaches it.
        let order_id = match status {
            Status::Created => None,
            Status::Expired => {
                expire(&outposts, &quote_id);
                None
            }
            Status::OrderSubmitted => {
                let order = outposts
                    .ok(
                        "POST",
                        "/orders",
                        Some(&json!({
                            "OutpostIdentifier": outpost_id.clone(),
                            "PaymentOption": "ALL_UPFRONT",
                            "QuoteIdentifier": quote_id.clone(),
                        })),
                    )
                    .await["Order"]
                    .clone();
                Some(text(&order, "OrderId"))
            }
        };

        // The quote really is in that status at the instant it is deleted, read the
        // way any client would read it.
        let before = outposts
            .ok("GET", &format!("/quotes/{quote_id}"), None)
            .await["Quote"]
            .clone();
        let reported = text(&before, "QuoteStatus");
        assert_eq!(
            reported,
            status.modelled(),
            "case {case:?}: expected the quote to be {} before the delete, got {reported:?}",
            status.modelled()
        );

        let identifier = form.of(&quote_id);
        let reply = delete_quote(&outposts, HOME_REGION, &identifier).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: DeleteQuote on {identifier:?} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert!(
            is_empty_payload(&reply.body),
            "case {case:?}: DeleteQuote's output declares no members, got {}",
            reply.body
        );

        // Gone to every later call: a read of either identifier form is not-found,
        // and a second delete is refused the same way.
        for form in [Form::BareId, Form::Arn] {
            let gone = form.of(&quote_id);
            let read = outposts
                .call("GET", &format!("/quotes/{}", segment(&gone)), None)
                .await;
            assert_eq!(
                (read.status, read.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: GetQuote on the deleted {gone:?} must be 404 \
                 NotFoundException, got {} {} {}",
                read.status,
                read.error_code,
                read.body
            );
            let again = delete_quote(&outposts, HOME_REGION, &gone).await;
            assert_eq!(
                (again.status, again.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: a second DeleteQuote on {gone:?} must be 404 \
                 NotFoundException, got {} {} {}",
                again.status,
                again.error_code,
                again.body
            );
        }

        // The list is the store's own answer about what it still holds.
        let listed = listed_ids(&outposts, HOME_REGION).await;
        assert!(
            !listed.contains(&quote_id),
            "case {case:?}: ListQuotes must not report the deleted {quote_id:?}, got {listed:?}"
        );
        assert!(
            listed.contains(&bystander_id),
            "case {case:?}: deleting one quote must leave {bystander_id:?} listed, got {listed:?}"
        );
        // And the bystander is still its own record, field for field.
        let bystander_read = outposts
            .ok("GET", &format!("/quotes/{bystander_id}"), None)
            .await["Quote"]
            .clone();
        assert_eq!(
            bystander_read, bystander,
            "case {case:?}: deleting one quote changed another one"
        );

        // An order submitted from the quote is a resource of its own: deleting the
        // quote it was priced on leaves the order readable, still naming that quote.
        if let Some(order_id) = order_id {
            let order = outposts
                .ok("GET", &format!("/orders/{order_id}"), None)
                .await["Order"]
                .clone();
            assert_eq!(
                text(&order, "QuoteIdentifier"),
                quote_id,
                "case {case:?}: the order must still name the quote it was submitted \
                 from, got {order}"
            );
        }

        // The Outpost a quote was associated with is untouched by the quote going
        // away: the association was the quote's, not the Outpost's.
        let outpost = outposts
            .ok("GET", &format!("/outposts/{outpost_id}"), None)
            .await["Outpost"]
            .clone();
        assert_eq!(
            text(&outpost, "OutpostId"),
            outpost_id,
            "case {case:?}: deleting a quote must leave its Outpost readable, got {outpost}"
        );
    }
}

/// Rule 2: an identifier that names no quote of this region is 404
/// `NotFoundException`; one that is no quote identifier at all is 400
/// `ValidationException`, from the boundary.
///
/// The rows span both identifier forms of an absent quote, the identifier of another
/// resource kind, and the ways a quote id can be malformed: wrong prefix, non-hex
/// suffix, uppercase hex, one character short of the modelled 17 and one too many.
#[tokio::test]
async fn deleting_an_absent_quote_is_not_found_and_a_malformed_identifier_is_refused() {
    let outposts = Outposts::new();
    let cases: Vec<Refusal> = vec![
        refusal(
            "DeleteQuote on a well-formed quote id that names nothing",
            "DELETE",
            format!("/quotes/{ABSENT_QUOTE_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "DeleteQuote on the ARN form of a quote that names nothing",
            "DELETE",
            format!(
                "/quotes/{}",
                segment(&format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"))
            ),
            404,
            "NotFoundException",
        ),
        refusal(
            "DeleteQuote on a label that is no identifier at all",
            "DELETE",
            "/quotes/not-a-quote-id".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteQuote on an Outpost id",
            "DELETE",
            format!("/quotes/{ABSENT_OUTPOST_ID}"),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteQuote on a quote id whose suffix is not hex",
            "DELETE",
            "/quotes/oq-zzzzzzzzzzzzzzzzz".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteQuote on a quote id whose hex is uppercase",
            "DELETE",
            "/quotes/oq-0000000000000000A".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteQuote on a quote id one hex character short",
            "DELETE",
            "/quotes/oq-0000000000000000".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "DeleteQuote on a quote id one hex character long",
            "DELETE",
            "/quotes/oq-000000000000000000".to_string(),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 2's message half, and its "removes nothing" half: a not-found delete is the
/// identifier ECHO `<member>=<value as supplied>` — the ARN form reads its ARN back,
/// not the bare id the service resolved it to — and the store it refused is exactly
/// what it was.
#[tokio::test]
async fn a_refused_delete_echoes_the_identifier_supplied_and_removes_nothing() {
    let outposts = Outposts::new();
    let kept = create_quote(
        &outposts,
        &quote_request(
            "US",
            json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 2.0 }]),
            json!({ "Description": "kept" }),
        ),
    )
    .await;
    let kept_id = text(&kept, "QuoteId");

    let cases: Vec<(&str, String, String)> = vec![
        (
            "an absent quote by bare id",
            ABSENT_QUOTE_ID.to_string(),
            format!("quoteIdentifier={ABSENT_QUOTE_ID}"),
        ),
        (
            "an absent quote by ARN",
            format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"),
            format!("quoteIdentifier={QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"),
        ),
    ];

    for (case, identifier, expected) in &cases {
        let reply = delete_quote(&outposts, HOME_REGION, identifier).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }

    // Nothing was removed: the one quote this store holds is still its own record.
    let listed = listed_ids(&outposts, HOME_REGION).await;
    assert_eq!(
        listed,
        BTreeSet::from([kept_id.clone()]),
        "a refused delete removes nothing, got {listed:?}"
    );
    let read = outposts
        .ok("GET", &format!("/quotes/{kept_id}"), None)
        .await["Quote"]
        .clone();
    assert_eq!(
        read, kept,
        "a refused delete must leave the quotes it did not name exactly as they were"
    );
}

/// Rule 3: a quote is part of its own region's inventory and of no other's, so
/// another region's endpoint cannot delete it — there the identifier names nothing —
/// and the quote stays readable and listed in the region it was created in.
///
/// The rows run the same quote content through both regions in both directions, and
/// through both identifier forms, so the scoping is the rule under test and not the
/// direction a row happened to pick.
#[tokio::test]
async fn a_quote_is_only_deletable_through_the_region_it_was_created_in() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, &str, &str, Form)> = vec![
        (
            "created at home, deleted from elsewhere by bare id",
            HOME_REGION,
            OTHER_REGION,
            Form::BareId,
        ),
        (
            "created at home, deleted from elsewhere by ARN",
            HOME_REGION,
            OTHER_REGION,
            Form::Arn,
        ),
        (
            "created elsewhere, deleted from home by bare id",
            OTHER_REGION,
            HOME_REGION,
            Form::BareId,
        ),
        (
            "created elsewhere, deleted from home by ARN",
            OTHER_REGION,
            HOME_REGION,
            Form::Arn,
        ),
    ];

    for (case, home, other, form) in &cases {
        let created = outposts
            .ok_in(
                home,
                "POST",
                "/quotes",
                Some(&quote_request(
                    "US",
                    json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                    json!({ "Description": "regional" }),
                )),
            )
            .await["Quote"]
            .clone();
        let quote_id = text(&created, "QuoteId");
        let identifier = form.of(&quote_id);

        let refused = delete_quote(&outposts, other, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: {other} must hold no quote {identifier:?}, got {} {} {}",
            refused.status,
            refused.error_code,
            refused.body
        );

        // Still there, in its own region.
        let read = outposts
            .ok_in(home, "GET", &format!("/quotes/{quote_id}"), None)
            .await["Quote"]
            .clone();
        assert_eq!(
            read, created,
            "case {case:?}: a delete addressed to {other} must leave {home}'s quote untouched"
        );
        assert!(
            listed_ids(&outposts, home).await.contains(&quote_id),
            "case {case:?}: {home} must still list {quote_id:?}"
        );

        // And its own region's endpoint does delete it.
        let deleted = delete_quote(&outposts, home, &identifier).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: {home} must delete its own quote, got {} {}",
            deleted.status, deleted.body
        );
        assert!(
            !listed_ids(&outposts, home).await.contains(&quote_id),
            "case {case:?}: {home} must no longer list the deleted {quote_id:?}"
        );
    }
}
