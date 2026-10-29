// SPDX-License-Identifier: CC0-1.0

//! A dimension key named more than once in one agreement request, end to end
//! over HTTP.
//!
//! The rule under test: a `dimensionKey` names ONE dimension of the agreement —
//! `ItemizedCharge$incrementalChargeAmount` is "the total incremental charge
//! amount for this dimension" and `oldQuantity` is "the existing quantity for
//! this dimension from the source agreement" — while `DimensionList` carries no
//! uniqueness rule at all (`smithy_query(query="shape", shape="DimensionList")`),
//! so a request may name one key several times, in one configuration or across
//! two configured prepaid terms. When it does, the quantity asked for is the
//! TOTAL of its occurrences, that dimension is itemized ONCE at that total, and
//! the quantity the source agreement already holds for it is subtracted from the
//! total exactly once: what is charged is (the total asked for) minus (the
//! existing quantity), at the published per-unit rate — never one subtraction
//! per occurrence. Accepting such a quote bills exactly that.
//!
//! The rows differ in what the rule generalises over: how many times a key is
//! repeated, whether the repetitions sit in one configuration or in two terms of
//! one request, whether the source agreement holds more, less or none of that
//! dimension (and whether the SOURCE itself names the key twice), which
//! dimension and contract length the repetitions buy, whether one repetition is
//! the zero quantity `ZeroValueInteger` admits, whether a distinct key is
//! interleaved with the repeated one, whether the request amends an agreement at
//! all, and whether the published rates price the repeated dimension. Every rate
//! is read back from the emulator, so no row can agree with a handler that
//! invents a price.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_over, call, create, emulator, new_request, upfront_term, Answer,
};

/// A product dimension the emulator's rate card publishes no rate for, so a
/// quote buying it carries no amount however often it is named.
const UNPUBLISHED_DIMENSION: &str = "Widgets";

/// A contract length the rate card publishes no rate at — a six-month term —
/// so a quote at it carries no amount however often a key is named.
const UNPUBLISHED_SELECTOR: &str = "P6M";

/// The minor units the amount in `value` spells, asserted to be money as this
/// service reports it: decimal units, a point and exactly two minor digits.
fn minor(case: &str, member: &str, value: &Value) -> u128 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must carry an amount, got {value:?}"));
    let (units, minor_digits) = text.split_once('.').unwrap_or_else(|| {
        panic!("case {case:?}: {member} must carry a decimal amount, got {text:?}")
    });
    assert_eq!(
        minor_digits.len(),
        2,
        "case {case:?}: {member} carries exactly two minor digits, got {text:?}"
    );
    let units: u128 = units
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {member} must carry decimal units, got {text:?}"));
    let minor_digits: u128 = minor_digits.parse().unwrap_or_else(|_| {
        panic!("case {case:?}: {member} must carry decimal minor units, got {text:?}")
    });
    units * 100 + minor_digits
}

/// `minor` minor units in the form the service reports an amount in.
fn as_amount(minor: u128) -> String {
    format!("{}.{:02}", minor / 100, minor % 100)
}

/// An amount member as minor units, or `None` when it is not reported at all —
/// which is what an unpriced dimension or charge carries.
fn optional_minor(case: &str, member: &str, value: &Value) -> Option<u128> {
    if value.is_null() {
        return None;
    }
    Some(minor(case, member, value))
}

/// `chargeSummary.expectedCharges`, in the order the quote reported them.
fn expected_charges(answer: &Answer) -> Vec<Value> {
    answer.body["chargeSummary"]["expectedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `chargeSummary.itemizedCharges`, in the order the quote reported them.
fn itemized_charges(answer: &Answer) -> Vec<Value> {
    answer.body["chargeSummary"]["itemizedCharges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The published per-unit rate of `key` at `selector`, READ BACK from the
/// emulator: what a new quote charges for one unit of it. `None` when the
/// published rates do not price it, in which case a quote buying it carries no
/// amount.
async fn unit_rate(router: &MultiServiceRouter, key: &str, selector: &str) -> Option<u128> {
    let case = "the published rate of one unit";
    let answer = create(
        router,
        new_request(
            "ap-repeatedrates",
            vec![upfront_term("cfg.rate", selector, &[(key, 1)])],
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: quoting one unit of {key} at {selector} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let items = itemized_charges(&answer);
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: one dimension is one line item, got {items:?}"
    );
    optional_minor(
        case,
        "itemizedCharges[0].incrementalChargeAmount",
        &items[0]["incrementalChargeAmount"],
    )
}

/// The same rate, looked up once per `(key, selector)` pair the rows name.
async fn rate_of(
    router: &MultiServiceRouter,
    rates: &mut BTreeMap<(String, String), Option<u128>>,
    key: &str,
    selector: &str,
) -> Option<u128> {
    let cached = rates.get(&(key.to_string(), selector.to_string())).copied();
    match cached {
        Some(rate) => rate,
        None => {
            let rate = unit_rate(router, key, selector).await;
            rates.insert((key.to_string(), selector.to_string()), rate);
            rate
        }
    }
}

/// One itemized charge as these rows compare it: `(dimensionKey, newQuantity,
/// oldQuantity, incrementalChargeAmount in minor units)`, the amount absent when
/// the published rates do not price the dimension.
type LineItem = (String, i64, i64, Option<u128>);

/// One row: what the source agreement holds, what the request asks for, and the
/// line items the rule says the quote must carry.
struct Row {
    case: &'static str,
    /// The contract length and the dimensions the source agreement was accepted
    /// over. Empty dimensions mean the row has no source at all: a NEW request,
    /// for which `oldQuantity` is the documented `0`.
    source: (&'static str, &'static [(&'static str, i32)]),
    /// One entry per requested term, in the order requested: its contract length
    /// and the dimensions its configuration names, repetitions included.
    requested: &'static [(&'static str, &'static [(&'static str, i32)])],
    /// The line items the quote must report for each requested term, in order:
    /// `(dimensionKey, newQuantity, oldQuantity)`.
    expected: &'static [&'static [(&'static str, i32, i32)]],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one key named twice in one configuration, over a source holding units of it",
            source: ("P12M", &[("Users", 4)]),
            requested: &[("P12M", &[("Users", 4), ("Users", 4)])],
            expected: &[&[("Users", 8, 4)]],
        },
        Row {
            case: "one key named three times, over a source that does not hold it",
            source: ("P12M", &[("Admins", 1)]),
            requested: &[("P12M", &[("Storage", 10), ("Storage", 20), ("Storage", 30)])],
            expected: &[&[("Storage", 60, 0)]],
        },
        Row {
            case: "a repeated key whose total stays below what the source already holds",
            source: ("P12M", &[("Users", 10)]),
            requested: &[("P12M", &[("Users", 3), ("Users", 4)])],
            expected: &[&[("Users", 7, 10)]],
        },
        Row {
            case: "a repeated key one of whose occurrences is the zero quantity admitted",
            source: ("P24M", &[("Nodes", 2)]),
            requested: &[("P24M", &[("Nodes", 0), ("Nodes", 5)])],
            expected: &[&[("Nodes", 5, 2)]],
        },
        Row {
            case: "a repeated key interleaved with a distinct one",
            source: ("P12M", &[("Users", 4), ("Admins", 1)]),
            requested: &[("P12M", &[("Users", 5), ("Admins", 2), ("Users", 6)])],
            expected: &[&[("Users", 11, 4), ("Admins", 2, 1)]],
        },
        Row {
            case: "two terms of one request naming the same key",
            source: ("P12M", &[("Users", 4)]),
            requested: &[("P12M", &[("Users", 4)]), ("P12M", &[("Users", 4)])],
            expected: &[&[("Users", 4, 4)], &[("Users", 4, 0)]],
        },
        Row {
            case: "a source that itself names one key twice, so it holds the total",
            source: ("P12M", &[("Users", 4), ("Users", 4)]),
            requested: &[("P12M", &[("Users", 10)])],
            expected: &[&[("Users", 10, 8)]],
        },
        Row {
            case: "a new agreement naming one key twice, so there is no existing quantity",
            source: ("P12M", &[]),
            requested: &[("P1M", &[("Hosts", 2), ("Hosts", 3)])],
            expected: &[&[("Hosts", 5, 0)]],
        },
        Row {
            case: "a repeated key at a contract length the published rates do not price",
            source: ("P12M", &[]),
            requested: &[(UNPUBLISHED_SELECTOR, &[("Users", 2), ("Users", 3)])],
            expected: &[&[("Users", 5, 0)]],
        },
        Row {
            case: "a repeated key the published rates do not price at all",
            source: ("P12M", &[]),
            requested: &[("P12M", &[(UNPUBLISHED_DIMENSION, 2), (UNPUBLISHED_DIMENSION, 2)])],
            expected: &[&[(UNPUBLISHED_DIMENSION, 4, 0)]],
        },
    ]
}

/// A repeated dimension key is quoted ONCE, at the total of its occurrences,
/// against the source agreement's existing quantity subtracted once.
#[tokio::test]
async fn a_repeated_dimension_key_is_quoted_once_at_the_total_of_its_occurrences() {
    let router = emulator();
    let mut rates: BTreeMap<(String, String), Option<u128>> = BTreeMap::new();

    for row in rows() {
        let case = row.case;
        let (source_selector, source_dimensions) = row.source;
        let source = if source_dimensions.is_empty() {
            None
        } else {
            Some(
                agreement_over(
                    &router,
                    "ap-repeated",
                    vec![upfront_term("cfg.source", source_selector, source_dimensions)],
                )
                .await,
            )
        };

        let terms: Vec<Value> = row
            .requested
            .iter()
            .enumerate()
            .map(|(index, (selector, dimensions))| {
                upfront_term(&format!("cfg.{index}"), selector, dimensions)
            })
            .collect();
        let request = match source.as_deref() {
            Some(id) => json!({
                "intent": "AMEND",
                "sourceAgreementIdentifier": id,
                "requestedTerms": terms,
            }),
            None => new_request("ap-repeated", terms),
        };
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the quote must succeed, got {} with {:?}",
            answer.status, answer.body
        );

        // What the rule implies, computed from the published rates and the row's
        // per-key totals rather than from the answer: each line item is charged
        // for the units above the existing quantity, each term's charge is the
        // sum of its line items, and the agreement value totals every unit the
        // request asks for ("the total value of the agreement, which includes
        // any amendments").
        let mut term_items: Vec<Vec<LineItem>> = Vec::new();
        let mut term_charges: Vec<Option<u128>> = Vec::new();
        let mut agreement_value: Option<u128> = Some(0);
        for ((selector, _), items) in row.requested.iter().zip(row.expected) {
            let mut charge: Option<u128> = Some(0);
            let mut expected_items = Vec::new();
            for (key, new_quantity, old_quantity) in *items {
                let rate = rate_of(&router, &mut rates, key, selector).await;
                let added =
                    u128::try_from(new_quantity.saturating_sub(*old_quantity)).unwrap_or(0);
                let bought = u128::try_from(*new_quantity).unwrap_or(0);
                let incremental = rate.map(|rate| rate * added);
                charge = charge.zip(incremental).map(|(total, line)| total + line);
                agreement_value = agreement_value
                    .zip(rate)
                    .map(|(total, rate)| total + rate * bought);
                expected_items.push((
                    key.to_string(),
                    i64::from(*new_quantity),
                    i64::from(*old_quantity),
                    incremental,
                ));
            }
            term_items.push(expected_items);
            term_charges.push(charge);
        }

        let charges = expected_charges(&answer);
        assert_eq!(
            charges.len(),
            row.requested.len(),
            "case {case:?}: one expected charge per configured prepaid term, got {:?}",
            answer.body["chargeSummary"]
        );
        let items = itemized_charges(&answer);
        let reported_count: usize = term_items.iter().map(Vec::len).sum();
        assert_eq!(
            items.len(),
            reported_count,
            "case {case:?}: one line item per dimension KEY, so {reported_count} of them, got {:?}",
            answer.body["chargeSummary"]
        );

        for (index, expected_items) in term_items.iter().enumerate() {
            let charge = &charges[index];
            let charge_id = charge["id"].as_str().unwrap_or_default().to_string();
            let reported: Vec<LineItem> = items
                .iter()
                .filter(|item| item["chargeReference"].as_str() == Some(charge_id.as_str()))
                .map(|item| {
                    (
                        item["dimensionKey"].as_str().unwrap_or_default().to_string(),
                        item["newQuantity"].as_i64().unwrap_or_default(),
                        item["oldQuantity"].as_i64().unwrap_or_default(),
                        optional_minor(
                            case,
                            "itemizedCharges[].incrementalChargeAmount",
                            &item["incrementalChargeAmount"],
                        ),
                    )
                })
                .collect();
            assert_eq!(
                &reported,
                expected_items,
                "case {case:?}: term {index} must itemize each dimension key once, at the total \
                 of its occurrences against the existing quantity subtracted once — expected \
                 {expected_items:?}, got {reported:?}"
            );

            let reported_charge = optional_minor(case, "expectedCharges[].amount", &charge["amount"]);
            assert_eq!(
                reported_charge,
                term_charges[index],
                "case {case:?}: term {index} is charged for the units it adds ({:?}), got {:?}",
                term_charges[index].map(as_amount),
                reported_charge.map(as_amount)
            );
        }

        let reported_value = answer.body["chargeSummary"]["newAgreementValue"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            reported_value,
            agreement_value.map(as_amount),
            "case {case:?}: the agreement value totals every unit asked for, got {:?}",
            answer.body["chargeSummary"]
        );
    }
}

/// Accepting an amendment that names one dimension twice bills the units it
/// adds: the charge is positive, so an invoice is issued for it, and the invoice
/// bills the amount the quote calculated.
#[tokio::test]
async fn accepting_an_amendment_over_a_repeated_key_bills_the_units_it_adds() {
    let router = emulator();
    let case = "an amendment naming one dimension twice over a source holding 4 Users";
    let users_rate = unit_rate(&router, "Users", "P12M")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: Users is published for P12M"));

    let source = agreement_over(
        &router,
        "ap-repeatedbilling",
        vec![upfront_term("cfg", "P12M", &[("Users", 4)])],
    )
    .await;

    let amended = create(
        &router,
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": source,
            "requestedTerms": [upfront_term("cfg", "P12M", &[("Users", 4), ("Users", 4)])],
        }),
    )
    .await;
    assert_eq!(
        amended.status, 200,
        "case {case:?}: the amendment must succeed, got {} with {:?}",
        amended.status, amended.body
    );
    let charges = expected_charges(&amended);
    let charged = minor(case, "expectedCharges[0].amount", &charges[0]["amount"]);
    let added = users_rate * 4;
    assert_eq!(
        charged,
        added,
        "case {case:?}: 8 Users over an agreement holding 4 adds 4 units, so {} — got {}",
        as_amount(added),
        as_amount(charged)
    );

    let request_id = amended.str_at("/agreementRequestId");
    let accepted = accept(&router, json!({"agreementRequestId": request_id})).await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: accepting the amendment must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement = accepted.str_at("/agreementId");

    let listed = call(
        &router,
        "ListAgreementInvoiceLineItems",
        json!({"agreementId": agreement, "groupBy": "INVOICE_ID"}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: listing the invoices must succeed, got {} with {:?}",
        listed.status, listed.body
    );
    let groups = listed.body["agreementInvoiceLineItemGroupSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: the amendment's charge is billed by one invoice, got {:?}",
        listed.body
    );
    let billed = minor(
        case,
        "agreementInvoiceLineItemGroupSummaries[0].pricingCurrencyAmount.amount",
        &groups[0]["pricingCurrencyAmount"]["amount"],
    );
    assert_eq!(
        billed,
        added,
        "case {case:?}: the invoice bills the units the amendment added ({}), got {}",
        as_amount(added),
        as_amount(billed)
    );
}
