// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateQuote`, driven over the wire through the same front
//! door as the rest of this crate's suites (whose harness it reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//!
//! > `UpdateQuote` applies exactly the members its request carries to the stored
//! > quote — replacing the stored value, including one the quote had no value for —
//! > leaves every member the request omits exactly as it was, and answers the record
//! > as it now stands, which is what every later read answers too.
//!
//! The Outpost association is the one member with three forms rather than two,
//! because the model widens it to `OutpostIdentifierOrEmpty` on this operation: an
//! omitted member leaves the association alone, an identifier (bare id or ARN) sets
//! it to that Outpost's own `OutpostArn`, and the empty string removes it.
//!
//! The rows differ along which members the request carries (none, one, several, all),
//! the value each carries (every value the modelled enums declare, a list at the
//! modelled `length(min: 0)`, free text at the modelled minimum and maximum), the
//! identifier form the request names the quote by (bare id and full quote ARN), and
//! whether the member had a value before the call.
//!
//! An identifier that names no quote is refused 404 `NotFoundException` with the
//! identifier echo, one that is no quote identifier at all 400 `ValidationException`,
//! and an `OutpostIdentifier` naming no Outpost 404 — and a refused update changes
//! nothing.

use serde_json::{json, Value};

use crate::quote_lifecycle::{create_quote, get_quote, request, ABSENT_QUOTE_ID, QUOTE_ARN_PREFIX};
use crate::site_and_outpost_lifecycle::{
    refusal, segment, text, without, Outposts, Refusal, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};
use crate::update_persistence::overlay;

/// Every value the modelled `PaymentOption` enum declares.
///
/// Source: `smithy_query(query="shape", shape="PaymentOption")` against this
/// service's model.
const PAYMENT_OPTIONS: &[&str] = &["ALL_UPFRONT", "NO_UPFRONT", "PARTIAL_UPFRONT"];

/// Every value the modelled `PaymentTerm` enum declares.
///
/// Source: `smithy_query(query="shape", shape="PaymentTerm")`.
const PAYMENT_TERMS: &[&str] = &["THREE_YEARS", "ONE_YEAR", "FIVE_YEARS"];

/// Every value the modelled `QuoteConstraintType` enum declares.
///
/// Source: `smithy_query(query="shape", shape="QuoteConstraintType")`.
const CONSTRAINT_TYPES: &[&str] = &[
    "RACK_MAXIMUM",
    "RACK_MAX_POWER_KVA",
    "RACK_MAX_WEIGHT_LBS",
    "RACK_SPACE_CONSTRAINED",
];

/// Every value the modelled `QuoteCapacityType` enum declares, with the unit that
/// type is measured in — the instance type for Amazon EC2, the storage unit for
/// Amazon EBS and Amazon S3.
///
/// Source: `smithy_query(query="shape", shape="QuoteCapacityType,QuoteCapacity")`.
const CAPACITY_TYPES: &[(&str, &str)] = &[("EC2", "c5.24xlarge"), ("EBS", "TiB"), ("S3", "TiB")];

/// `PATCH /quotes/{QuoteIdentifier}`, returning the `Quote` it answered with.
async fn update_quote(outposts: &Outposts, identifier: &str, patch: &Value) -> Value {
    outposts
        .ok(
            "PATCH",
            &format!("/quotes/{}", segment(identifier)),
            Some(patch),
        )
        .await["Quote"]
        .clone()
}

/// One `RequestedCapacities` list carrying every modelled capacity type.
fn every_capacity() -> Value {
    CAPACITY_TYPES
        .iter()
        .enumerate()
        .map(|(index, (capacity_type, unit))| {
            json!({
                "QuoteCapacityType": capacity_type,
                "Unit": unit,
                "Quantity": (index + 1) as f64,
            })
        })
        .collect()
}

/// One `RequestedConstraints` list carrying every modelled constraint type.
fn every_constraint() -> Value {
    CONSTRAINT_TYPES
        .iter()
        .map(|constraint_type| json!({ "QuoteConstraintType": constraint_type, "Value": "1" }))
        .collect()
}

/// The quote a case starts from: one carrying a value for every member an update can
/// write, so each row below overwrites a member that already had one.
async fn a_specified_quote(outposts: &Outposts) -> Value {
    create_quote(
        outposts,
        &request(
            "US",
            json!([{ "QuoteCapacityType": "EC2", "Unit": "c5.24xlarge", "Quantity": 2.0 }]),
            json!({
                "RequestedConstraints": [{ "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" }],
                "RequestedPaymentOptions": ["ALL_UPFRONT"],
                "RequestedPaymentTerms": ["THREE_YEARS"],
                "Description": "created description",
            }),
        ),
    )
    .await
}

/// The rule's main form: each row's members land on the stored quote, every member it
/// omits is untouched, and `GetQuote` answers the updated record by either identifier
/// form.
///
/// The rows are applied in sequence to ONE quote, so a row that left a previous row's
/// write behind, or that wrote a member it never named, shows up immediately.
#[tokio::test]
async fn update_quote_applies_present_members_and_reads_back() {
    let outposts = Outposts::new();
    let created = a_specified_quote(&outposts).await;
    let quote_id = text(&created, "QuoteId");
    let quote_arn = format!("{QUOTE_ARN_PREFIX}{quote_id}");

    let cases: Vec<(&str, &str, Value)> = vec![
        ("no updatable member, by id", quote_id.as_str(), json!({})),
        (
            "no updatable member, by ARN",
            quote_arn.as_str(),
            json!({}),
        ),
        (
            "country code only",
            quote_id.as_str(),
            json!({ "CountryCode": "GB" }),
        ),
        (
            "description only",
            quote_id.as_str(),
            json!({ "Description": "re-described" }),
        ),
        (
            "the empty description the modelled length(min: 0) allows",
            quote_id.as_str(),
            json!({ "Description": "" }),
        ),
        (
            "description at the modelled 1024-character maximum",
            quote_id.as_str(),
            json!({ "Description": "d".repeat(1024) }),
        ),
        (
            "a multi-line description, named by ARN",
            quote_arn.as_str(),
            json!({ "Description": "first line\nsecond line" }),
        ),
        (
            "requested capacities replaced with every modelled capacity type",
            quote_id.as_str(),
            json!({ "RequestedCapacities": every_capacity() }),
        ),
        (
            "requested capacities emptied to the modelled length(min: 0)",
            quote_id.as_str(),
            json!({ "RequestedCapacities": [] }),
        ),
        (
            "every modelled constraint type",
            quote_id.as_str(),
            json!({ "RequestedConstraints": every_constraint() }),
        ),
        (
            "constraints emptied to the modelled length(min: 0)",
            quote_id.as_str(),
            json!({ "RequestedConstraints": [] }),
        ),
        (
            "every payment option the model declares",
            quote_id.as_str(),
            json!({ "RequestedPaymentOptions": PAYMENT_OPTIONS }),
        ),
        (
            "every payment term the model declares",
            quote_id.as_str(),
            json!({ "RequestedPaymentTerms": PAYMENT_TERMS }),
        ),
        (
            "one payment option and one term, the last of each",
            quote_id.as_str(),
            json!({
                "RequestedPaymentOptions": [PAYMENT_OPTIONS[PAYMENT_OPTIONS.len() - 1]],
                "RequestedPaymentTerms": [PAYMENT_TERMS[PAYMENT_TERMS.len() - 1]],
            }),
        ),
        (
            "every updatable member at once, named by ARN",
            quote_arn.as_str(),
            json!({
                "CountryCode": "JP",
                "Description": "everything at once",
                "RequestedCapacities": every_capacity(),
                "RequestedConstraints": every_constraint(),
                "RequestedPaymentOptions": PAYMENT_OPTIONS,
                "RequestedPaymentTerms": PAYMENT_TERMS,
            }),
        ),
    ];

    let mut expected = created;
    for (case, identifier, patch) in &cases {
        expected = overlay(&expected, patch);
        let answered = update_quote(&outposts, identifier, patch).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: UpdateQuote must answer the updated quote, got {answered:?}"
        );
        assert_eq!(
            get_quote(&outposts, &quote_id).await,
            expected,
            "case {case:?}: GetQuote by id must answer the updated quote"
        );
        assert_eq!(
            get_quote(&outposts, &quote_arn).await,
            expected,
            "case {case:?}: GetQuote by ARN must answer the updated quote"
        );
    }
}

/// The association half of the rule, over every form the modelled
/// `OutpostIdentifierOrEmpty` admits: an identifier sets the association to that
/// Outpost's own `OutpostArn`, the empty string removes it, and an omitted member
/// leaves whatever the quote already had.
///
/// The rows span a quote that starts with NO association (the case the member has to
/// appear for), both identifier forms, replacing one association with another,
/// removing it, removing it twice, and setting it again afterwards.
#[tokio::test]
async fn update_quote_sets_and_removes_the_outpost_association() {
    let outposts = Outposts::new();
    let (first_id, first_arn) = outposts.site_and_outpost("quote-update-first").await;
    let (second_id, second_arn) = outposts.site_and_outpost("quote-update-second").await;
    let created = create_quote(&outposts, &request("US", json!([]), json!({}))).await;
    let quote_id = text(&created, "QuoteId");
    assert!(
        created.get("OutpostArn").is_none(),
        "a quote created with no Outpost reports none, got {created:?}"
    );

    // Each row: the `OutpostIdentifier` the request carries (absent for `None`), and
    // the `OutpostArn` the quote must report afterwards (absent for `None`).
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        (
            "no OutpostIdentifier leaves the unassociated quote unassociated",
            None,
            None,
        ),
        (
            "associated by bare Outpost id",
            Some(first_id.as_str()),
            Some(first_arn.as_str()),
        ),
        (
            "no OutpostIdentifier leaves the association alone",
            None,
            Some(first_arn.as_str()),
        ),
        (
            "re-associated with another Outpost by ARN",
            Some(second_arn.as_str()),
            Some(second_arn.as_str()),
        ),
        (
            "re-associated with the first Outpost by bare id",
            Some(first_id.as_str()),
            Some(first_arn.as_str()),
        ),
        (
            "the empty string removes the association",
            Some(""),
            None,
        ),
        (
            "the empty string again, on a quote that has no association",
            Some(""),
            None,
        ),
        (
            "associated again after removal, by ARN",
            Some(second_arn.as_str()),
            Some(second_arn.as_str()),
        ),
    ];

    for (case, identifier, expected_arn) in &cases {
        let patch = match identifier {
            Some(identifier) => json!({ "OutpostIdentifier": identifier }),
            None => json!({}),
        };
        let answered = update_quote(&outposts, &quote_id, &patch).await;
        assert_eq!(
            answered.get("OutpostArn").and_then(Value::as_str),
            *expected_arn,
            "case {case:?}: expected OutpostArn {expected_arn:?}, got {answered:?}"
        );
        // The association is the only member the request named, so nothing else may
        // have moved: the rest of the quote is exactly what the create reported.
        assert_eq!(
            without(&answered, "OutpostArn"),
            without(&created, "OutpostArn"),
            "case {case:?}: setting the association changed another member, got {answered:?}"
        );
        assert_eq!(
            get_quote(&outposts, &quote_id).await,
            answered,
            "case {case:?}: GetQuote must answer the association the update reported"
        );
    }

    // The Outposts the association named are untouched by it: an association is
    // recorded on the quote, not on the Outpost.
    for (case, outpost_id) in [("first", &first_id), ("second", &second_id)] {
        let outpost = outposts.get_outpost(outpost_id).await;
        assert_eq!(
            text(&outpost, "OutpostId"),
            *outpost_id,
            "case {case:?}: the Outpost a quote was associated with still reads back, got {outpost:?}"
        );
    }
}

/// The rule stated over the whole store: an update writes to the quote it names and
/// to no other, whichever identifier form named it.
#[tokio::test]
async fn updating_one_quote_leaves_every_other_quote_alone() {
    let outposts = Outposts::new();
    let bodies: Vec<(&str, Value)> = vec![
        ("no capacities", request("US", json!([]), json!({}))),
        (
            "one capacity and a description",
            request(
                "GB",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "c5.24xlarge", "Quantity": 1.0 }]),
                json!({ "Description": "second" }),
            ),
        ),
        (
            "every capacity type",
            request("IE", every_capacity(), json!({})),
        ),
    ];

    let mut quotes: Vec<(&str, Value)> = Vec::new();
    for (case, body) in &bodies {
        quotes.push((case, create_quote(&outposts, body).await));
    }

    for index in 0..quotes.len() {
        let target = text(&quotes[index].1, "QuoteId");
        let patch = json!({ "Description": format!("updated {index}"), "CountryCode": "DE" });
        let answered = update_quote(&outposts, &target, &patch).await;
        let expected = overlay(&quotes[index].1, &patch);
        assert_eq!(
            answered, expected,
            "case {:?}: UpdateQuote must answer the updated quote",
            quotes[index].0
        );
        quotes[index].1 = expected;

        for (case, quote) in &quotes {
            let read = get_quote(&outposts, &text(quote, "QuoteId")).await;
            assert_eq!(
                &read, quote,
                "case {case:?}: updating {target} must leave every other quote as it was"
            );
        }
    }
}

/// An update whose quote identifier names nothing is 404 `NotFoundException`, one
/// whose identifier is no quote identifier at all is 400 `ValidationException`, and
/// one naming an Outpost that does not exist is 404 — and the refusal is the
/// identifier echo of the value AS SUPPLIED, naming the member it arrived in.
#[tokio::test]
async fn an_update_of_an_absent_quote_or_outpost_is_refused() {
    let outposts = Outposts::new();
    let created = a_specified_quote(&outposts).await;
    let quote_id = text(&created, "QuoteId");
    let absent_quote_arn = format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}");
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let cases: Vec<Refusal> = vec![
        refusal(
            "a well-formed quote id that names nothing",
            "PATCH",
            format!("/quotes/{ABSENT_QUOTE_ID}"),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "Description": "no such quote" })),
        refusal(
            "the ARN form of a quote that names nothing",
            "PATCH",
            format!("/quotes/{}", segment(&absent_quote_arn)),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "CountryCode": "US" })),
        refusal(
            "a label that is no identifier at all",
            "PATCH",
            "/quotes/not-a-quote-id".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Description": "nope" })),
        refusal(
            "an Outpost id in the quote identifier",
            "PATCH",
            format!("/quotes/{ABSENT_OUTPOST_ID}"),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Description": "nope" })),
        refusal(
            "a quote id whose suffix is not hex",
            "PATCH",
            "/quotes/oq-zzzzzzzzzzzzzzzzz".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Description": "nope" })),
        refusal(
            "a quote id one hex character short",
            "PATCH",
            "/quotes/oq-0000000000000000".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Description": "nope" })),
        refusal(
            "a quote id one hex character long",
            "PATCH",
            "/quotes/oq-000000000000000000".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({ "Description": "nope" })),
        refusal(
            "an existing quote, naming an Outpost that does not exist",
            "PATCH",
            format!("/quotes/{quote_id}"),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID })),
        refusal(
            "an existing quote, naming the ARN of an Outpost that does not exist",
            "PATCH",
            format!("/quotes/{quote_id}"),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "OutpostIdentifier": absent_outpost_arn.clone() })),
    ];

    outposts.expect_refusals(&cases).await;

    // A refused update wrote nothing: the quote is still exactly what its create
    // reported, and the absent quote was not created to satisfy the request.
    assert_eq!(
        get_quote(&outposts, &quote_id).await,
        created,
        "a refused update must leave the quote exactly as it was"
    );
    let reply = outposts
        .call("GET", &format!("/quotes/{ABSENT_QUOTE_ID}"), None)
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "NotFoundException"),
        "a refused update must not have created the quote, got {} {:?}",
        reply.status,
        reply.body
    );

    // The refusal's message half: the identifier echo of the value AS SUPPLIED, under
    // the member it arrived in.
    let echoes: Vec<(&str, String, Value, String)> = vec![
        (
            "the quote identifier, by bare id",
            format!("/quotes/{ABSENT_QUOTE_ID}"),
            json!({ "Description": "no such quote" }),
            format!("quoteIdentifier={ABSENT_QUOTE_ID}"),
        ),
        (
            "the quote identifier, by ARN",
            format!("/quotes/{}", segment(&absent_quote_arn)),
            json!({ "Description": "no such quote" }),
            format!("quoteIdentifier={absent_quote_arn}"),
        ),
        (
            "the Outpost identifier, by bare id",
            format!("/quotes/{quote_id}"),
            json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID }),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "the Outpost identifier, by ARN",
            format!("/quotes/{quote_id}"),
            json!({ "OutpostIdentifier": absent_outpost_arn.clone() }),
            format!("outpostIdentifier={absent_outpost_arn}"),
        ),
    ];

    for (case, path, body, expected) in &echoes {
        let reply = outposts.call("PATCH", path, Some(body)).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }
}
