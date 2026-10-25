// SPDX-License-Identifier: CC0-1.0

//! The rate card of a PURCHASED Savings Plan, end to end over the wire.
//!
//! Four rules, each asserted over a table of plans and searches rather than over
//! the one row a recording happened to carry:
//!
//! * a plan's rate card is every usage line item its own commitment covers,
//!   priced exactly as the offering it was bought from prices that line item —
//!   so the card is the offering's rate card, fixed at purchase, and a deeper
//!   commitment prices the same usage strictly lower;
//! * every filter the request carries narrows the card, and a line item that is
//!   not dimensioned by the property a filter names cannot satisfy it;
//! * the card is paged in one stable order: paging yields it once, and a caller
//!   that names no page size gets the service's own page and a cursor;
//! * an id this account never purchased has no rate card — a 404, not an empty
//!   one — and a cursor from another search is refused.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, so
//! what is pinned is the HTTP behaviour. The harness (`Emulator`, `text`) is the
//! one the sibling lifecycle suite defines.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::ops::types::{
    SAVINGS_PLAN_RATE_PROPERTY_KEY_VALUES, SAVINGS_PLAN_RATE_SERVICE_CODE_VALUES,
    SAVINGS_PLAN_RATE_UNIT_VALUES,
};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};

/// The operation under test.
const OPERATION: &str = "DescribeSavingsPlanRates";

/// The sibling search a plan's card is compared against: what the offering the
/// plan was bought from charges for the same line items.
const OFFERING_RATES: &str = "DescribeSavingsPlansOfferingRates";

/// The largest page either search admits (`MaxResults`/`PageSize` top out at
/// 1000), used to walk a whole card in as few calls as possible.
const LARGEST_PAGE: i64 = 1000;

/// What distinguishes one line item of a rate card from another: the product,
/// the service that bills it, its usage type and its billing operation.
fn key(rate: &Value) -> String {
    format!(
        "{}|{}|{}|{}",
        text(rate, "productType"),
        text(rate, "serviceCode"),
        text(rate, "usageType"),
        text(rate, "operation")
    )
}

/// The dimensions a rate is priced along, as `name -> value`.
fn properties(rate: &Value) -> BTreeMap<String, String> {
    rate.get("properties")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|property| (text(property, "name"), text(property, "value")))
                .collect()
        })
        .unwrap_or_default()
}

/// The field a `SavingsPlanRateFilterName` names on a rate: one of the rate's
/// own billing dimensions, or the property of that name — `None` when the rate
/// is not dimensioned by it.
fn filter_field(rate: &Value, name: &str) -> Option<String> {
    match name {
        "productType" | "serviceCode" | "usageType" | "operation" => Some(text(rate, name)),
        property => properties(rate).get(property).cloned(),
    }
}

/// A rate as a whole number of [`state::AMOUNT_SCALE`] units, which is also an
/// assertion that it is spelled at the service's one fixed scale.
fn rate_units(case: &str, rate: &Value) -> i128 {
    let rendered = text(rate, "rate");
    let Some((whole, fraction)) = rendered.split_once('.') else {
        panic!("case {case:?}: a rate must be a decimal amount, got {rendered:?}");
    };
    assert_eq!(
        fraction.len(),
        state::AMOUNT_WIRE_DIGITS,
        "case {case:?}: a rate must carry {} fractional digits, got {rendered:?}",
        state::AMOUNT_WIRE_DIGITS
    );
    let whole: i128 = whole
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {rendered:?} is not a decimal amount"));
    let fraction: i128 = fraction
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {rendered:?} is not a decimal amount"));
    whole * state::AMOUNT_SCALE + fraction
}

/// Every page of a plan-rate search, as `(rows, nextToken)`. `request` is sent
/// as supplied for the first page and with `nextToken` added for each page after
/// it.
async fn pages(
    emulator: &Emulator,
    case: &str,
    request: &Value,
) -> Vec<(Vec<Value>, Option<String>)> {
    let mut pages: Vec<(Vec<Value>, Option<String>)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next {
            page_request["nextToken"] = json!(token);
        }
        let answer = emulator.call(OPERATION, page_request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {page_request}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            text(&answer.body, "savingsPlanId"),
            text(request, "savingsPlanId"),
            "case {case:?}: the answer must name the plan the request asked about, got {:?}",
            answer.body
        );
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        pages.push((answer.list("searchResults").to_vec(), next.clone()));
        assert!(
            pages.len() <= 400,
            "case {case:?}: paging did not terminate after {} pages",
            pages.len()
        );
        if next.is_none() {
            return pages;
        }
    }
}

/// The whole rate card a plan-rate search admits, walked through its own cursor
/// in pages of [`LARGEST_PAGE`].
async fn card(emulator: &Emulator, case: &str, request: &Value) -> Vec<Value> {
    let mut request = request.clone();
    request["maxResults"] = json!(LARGEST_PAGE);
    pages(emulator, case, &request)
        .await
        .into_iter()
        .flat_map(|(rows, _)| rows)
        .collect()
}

/// Every rate the OFFERING `offering_id` prices, walked through that search's
/// own cursor.
async fn offering_card(emulator: &Emulator, case: &str, offering_id: &str) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({
            "savingsPlanOfferingIds": [offering_id],
            "maxResults": LARGEST_PAGE,
        });
        if let Some(token) = &next {
            request["nextToken"] = json!(token);
        }
        let answer = emulator.call(OFFERING_RATES, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {request}, got {} {:?}",
            answer.status, answer.body
        );
        rows.extend(answer.list("searchResults").iter().cloned());
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if next.is_none() {
            return rows;
        }
    }
}

/// Buy one offering of this plan type, payment option and term, and answer
/// `(savingsPlanId, offering, plan)`.
async fn buy(
    emulator: &Emulator,
    plan_type: &str,
    payment_option: &str,
    duration: i64,
    commitment: &str,
) -> (String, Value, Value) {
    let offering = emulator.offering(plan_type, payment_option, duration).await;
    let offering_id = text(&offering, "offeringId");
    let savings_plan_id = emulator
        .purchase(json!({
            "savingsPlanOfferingId": offering_id,
            "commitment": commitment,
        }))
        .await;
    let plan = emulator.plan(&savings_plan_id).await;
    (savings_plan_id, offering, plan)
}

/// The `productTypes` a plan reports.
fn product_types(plan: &Value) -> BTreeSet<String> {
    plan.get("productTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|product| product.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The plans the tables below buy: one per `SavingsPlanType` the catalog sells,
/// each at a different payment option and term, so the rows differ in how wide
/// the plan's coverage is (one product, a family of them, or several), in
/// whether it is scoped to a region and an instance family, and in how deep a
/// discount it earns.
const PLANS: [(&str, &str, i64, &str); 4] = [
    ("Compute", state::PAYMENT_NO_UPFRONT, ONE_YEAR, "0.5"),
    ("EC2Instance", state::PAYMENT_ALL_UPFRONT, THREE_YEARS, "1.25"),
    ("SageMaker", state::PAYMENT_PARTIAL_UPFRONT, ONE_YEAR, "0.1"),
    ("Database", state::PAYMENT_ALL_UPFRONT, THREE_YEARS, "2"),
];

/// A plan's rate card is every line item its commitment covers, priced exactly
/// as the offering it was bought from prices that line item.
///
/// The expected card is not a list written down here: it is the sibling
/// offering-rate search's answer for the SAME offering, so the row pins "a
/// plan's rates are the rates of its offering" over four plans that differ in
/// coverage, scope, term and payment option.
#[tokio::test]
async fn a_plans_rate_card_is_the_rate_card_of_its_offering() {
    let emulator = Emulator::new();

    for (plan_type, payment_option, duration, commitment) in PLANS {
        let case = format!("{plan_type}/{payment_option}/{duration}");
        let (savings_plan_id, offering, plan) =
            buy(&emulator, plan_type, payment_option, duration, commitment).await;
        let rates = card(
            &emulator,
            &case,
            &json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
        assert!(
            !rates.is_empty(),
            "case {case:?}: a purchased plan must have a rate card, got none"
        );

        let expected: BTreeMap<String, Value> = offering_card(
            &emulator,
            &case,
            &text(&offering, "offeringId"),
        )
        .await
        .into_iter()
        .map(|rate| (key(&rate), rate))
        .collect();
        let found: BTreeMap<String, Value> = rates
            .iter()
            .map(|rate| (key(rate), rate.clone()))
            .collect();
        assert_eq!(
            found.len(),
            rates.len(),
            "case {case:?}: a line item must be priced once in a card, got {} rows for {} line items",
            rates.len(),
            found.len()
        );
        assert_eq!(
            found.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "case {case:?}: the plan must price exactly the line items its offering prices"
        );

        for (line_item, rate) in &found {
            let offering_rate = &expected[line_item];
            assert_eq!(
                rate_units(&case, rate),
                rate_units(&case, offering_rate),
                "case {case:?} {line_item:?}: expected the offering's rate {:?}, got {:?}",
                text(offering_rate, "rate"),
                text(rate, "rate")
            );
            assert_eq!(
                properties(rate),
                properties(offering_rate),
                "case {case:?} {line_item:?}: expected the offering's dimensions, got {:?}",
                properties(rate)
            );
            assert_eq!(
                text(rate, "currency"),
                text(&plan, "currency"),
                "case {case:?} {line_item:?}: expected the plan's currency {:?}, got {:?}",
                text(&plan, "currency"),
                text(rate, "currency")
            );
            // The wire vocabulary of a row is the model's own value sets.
            assert!(
                SAVINGS_PLAN_RATE_UNIT_VALUES.contains(&text(rate, "unit").as_str()),
                "case {case:?} {line_item:?}: {:?} is not a modelled rate unit",
                text(rate, "unit")
            );
            assert!(
                SAVINGS_PLAN_RATE_SERVICE_CODE_VALUES
                    .contains(&text(rate, "serviceCode").as_str()),
                "case {case:?} {line_item:?}: {:?} is not a modelled service code",
                text(rate, "serviceCode")
            );
            for name in properties(rate).keys() {
                assert!(
                    SAVINGS_PLAN_RATE_PROPERTY_KEY_VALUES.contains(&name.as_str()),
                    "case {case:?} {line_item:?}: {name:?} is not a modelled property key"
                );
            }
            // A card covers only what the plan itself covers: its products, and
            // the region and instance family it is scoped to where it has one.
            assert!(
                product_types(&plan).contains(&text(rate, "productType")),
                "case {case:?} {line_item:?}: expected a product the plan covers ({:?}), got {:?}",
                product_types(&plan),
                text(rate, "productType")
            );
            for (member, property) in [
                ("region", state::PROPERTY_REGION),
                ("ec2InstanceFamily", state::PROPERTY_INSTANCE_FAMILY),
            ] {
                let scope = text(&plan, member);
                if !scope.is_empty() {
                    assert_eq!(
                        properties(rate).get(property).cloned().unwrap_or_default(),
                        scope,
                        "case {case:?} {line_item:?}: a plan scoped to {member} {scope:?} must price only that, got {:?}",
                        properties(rate)
                    );
                }
            }
        }

        // A read leaves the card where it was: the same request answers the same
        // way for the next caller.
        let again = card(
            &emulator,
            &case,
            &json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
        assert_eq!(
            again, rates,
            "case {case:?}: reading a rate card must not change it"
        );
    }
}

/// A deeper commitment prices the same usage strictly lower: the rate a plan
/// charges falls as its term lengthens and as more of the commitment is paid up
/// front.
///
/// The rows are EC2 Instance plans bought from the same scope at every term and
/// payment option the catalog sells, compared on every line item their cards
/// share — so what is pinned is the ordering over the whole card, not one row's
/// value.
#[tokio::test]
async fn a_deeper_commitment_prices_a_plans_usage_lower() {
    let emulator = Emulator::new();
    // Shallowest commitment first: a one-year plan that pays nothing up front,
    // then more paid up front, then the same ladder over a three-year term.
    let ladder = [
        (ONE_YEAR, state::PAYMENT_NO_UPFRONT),
        (ONE_YEAR, state::PAYMENT_PARTIAL_UPFRONT),
        (ONE_YEAR, state::PAYMENT_ALL_UPFRONT),
        (THREE_YEARS, state::PAYMENT_NO_UPFRONT),
        (THREE_YEARS, state::PAYMENT_PARTIAL_UPFRONT),
        (THREE_YEARS, state::PAYMENT_ALL_UPFRONT),
    ];

    let mut previous: Option<(String, BTreeMap<String, i128>)> = None;
    for (duration, payment_option) in ladder {
        let case = format!("EC2Instance/{payment_option}/{duration}");
        let (savings_plan_id, _, _) =
            buy(&emulator, "EC2Instance", payment_option, duration, "0.1").await;
        let priced: BTreeMap<String, i128> = card(
            &emulator,
            &case,
            &json!({ "savingsPlanId": savings_plan_id }),
        )
        .await
        .iter()
        .map(|rate| (key(rate), rate_units(&case, rate)))
        .collect();
        assert!(
            !priced.is_empty(),
            "case {case:?}: expected a rate card, got none"
        );

        if let Some((earlier_case, earlier)) = &previous {
            for (line_item, units) in &priced {
                let Some(earlier_units) = earlier.get(line_item) else {
                    continue;
                };
                assert!(
                    units < earlier_units,
                    "case {case:?} {line_item:?}: a deeper commitment must price it below \
                     {earlier_case:?}'s {earlier_units}, got {units}"
                );
            }
        }
        previous = Some((case, priced));
    }
}

/// Every filter the request carries narrows the card, and a line item that is
/// not dimensioned by the property a filter names cannot satisfy it.
///
/// Each row's expected answer is computed from the UNFILTERED card by the same
/// predicate the rule states, so a row pins "the filter admits exactly these
/// line items" rather than a count someone wrote down. The rows differ along
/// which filter name is used (a billing dimension or a priced property), how
/// many values it names, whether a value matches nothing, and whether the card's
/// line items are dimensioned by the property at all.
#[tokio::test]
async fn every_filter_narrows_the_rate_card() {
    let emulator = Emulator::new();

    for (plan_type, payment_option, duration, commitment) in PLANS {
        let plan_case = format!("{plan_type}/{payment_option}/{duration}");
        let (savings_plan_id, _, _) =
            buy(&emulator, plan_type, payment_option, duration, commitment).await;
        let unfiltered = card(
            &emulator,
            &plan_case,
            &json!({ "savingsPlanId": savings_plan_id }),
        )
        .await;
        assert!(
            !unfiltered.is_empty(),
            "case {plan_case:?}: expected a rate card, got none"
        );

        // One value of each filter name taken from the card itself, so the row
        // is a filter the card can satisfy, plus the names and values it cannot.
        let sample = |name: &str| -> Option<String> {
            unfiltered
                .iter()
                .find_map(|rate| filter_field(rate, name).filter(|value| !value.is_empty()))
        };
        let mut cases: Vec<(String, Vec<String>)> = Vec::new();
        // `SavingsPlanRateFilterName`, as the model declares it.
        for name in [
            "region",
            "instanceType",
            "productDescription",
            "tenancy",
            "productType",
            "serviceCode",
            "usageType",
            "operation",
        ] {
            cases.push((name.to_string(), sample(name).into_iter().collect()));
            // A value no line item carries admits nothing; a filter naming no
            // value at all constrains nothing.
            cases.push((name.to_string(), vec!["no-such-value".to_string()]));
            cases.push((name.to_string(), Vec::new()));
        }
        // Several values of one filter admit the union of what each admits.
        let mut regions: Vec<String> = unfiltered
            .iter()
            .filter_map(|rate| filter_field(rate, "region"))
            .collect();
        regions.sort();
        regions.dedup();
        regions.truncate(2);
        if regions.len() == 2 {
            cases.push(("region".to_string(), regions));
        }
        let mut units: Vec<String> = unfiltered
            .iter()
            .filter_map(|rate| filter_field(rate, "serviceCode"))
            .collect();
        units.sort();
        units.dedup();
        units.truncate(3);
        cases.push(("serviceCode".to_string(), units));

        for (name, values) in cases {
            let case = format!("{plan_case} filter {name}={values:?}");
            let expected: Vec<String> = unfiltered
                .iter()
                .filter(|rate| {
                    values.is_empty()
                        || filter_field(rate, &name).is_some_and(|field| values.contains(&field))
                })
                .map(key)
                .collect();
            let found: Vec<String> = card(
                &emulator,
                &case,
                &json!({
                    "savingsPlanId": savings_plan_id,
                    "filters": [{ "name": name, "values": values }],
                }),
            )
            .await
            .iter()
            .map(key)
            .collect();
            assert_eq!(
                found, expected,
                "case {case:?}: expected the {} line items the filter admits, got {}",
                expected.len(),
                found.len()
            );
        }

        // Two filters narrow together: only the line items both admit.
        if let (Some(region), Some(product)) = (
            unfiltered.iter().find_map(|rate| filter_field(rate, "region")),
            unfiltered
                .iter()
                .find_map(|rate| filter_field(rate, "productType")),
        ) {
            let case = format!("{plan_case} filter region={region} productType={product}");
            let expected: Vec<String> = unfiltered
                .iter()
                .filter(|rate| {
                    filter_field(rate, "region").as_deref() == Some(region.as_str())
                        && filter_field(rate, "productType").as_deref() == Some(product.as_str())
                })
                .map(key)
                .collect();
            let found: Vec<String> = card(
                &emulator,
                &case,
                &json!({
                    "savingsPlanId": savings_plan_id,
                    "filters": [
                        { "name": "region", "values": [region] },
                        { "name": "productType", "values": [product] },
                    ],
                }),
            )
            .await
            .iter()
            .map(key)
            .collect();
            assert_eq!(
                found, expected,
                "case {case:?}: expected the {} line items both filters admit, got {}",
                expected.len(),
                found.len()
            );
        }
    }
}

/// The card is paged in one stable order: every page size yields the same card
/// once, a caller that names no size gets the service's own page and a cursor to
/// resume, and the last page of a walk carries none.
///
/// The rows are two cards of very different size — one scoped plan whose card is
/// a handful of line items, and one unscoped plan whose card is larger than the
/// service's own page — paged at sizes below, at and above the card's size.
#[tokio::test]
async fn a_rate_card_is_paged_once_in_a_stable_order() {
    let emulator = Emulator::new();

    let (small, _, _) = buy(
        &emulator,
        "Database",
        state::PAYMENT_NO_UPFRONT,
        ONE_YEAR,
        "0.5",
    )
    .await;
    let (large, _, _) = buy(
        &emulator,
        "Compute",
        state::PAYMENT_ALL_UPFRONT,
        THREE_YEARS,
        "0.5",
    )
    .await;

    for (label, savings_plan_id, sizes) in [
        ("a scoped plan", small, vec![1_i64, 2, 3, 7, LARGEST_PAGE]),
        ("an unscoped plan", large, vec![499_i64, LARGEST_PAGE]),
    ] {
        let case = format!("{label} paged");
        let whole: Vec<String> = card(
            &emulator,
            &case,
            &json!({ "savingsPlanId": savings_plan_id }),
        )
        .await
        .iter()
        .map(key)
        .collect();
        assert!(
            !whole.is_empty(),
            "case {case:?}: expected a rate card, got none"
        );

        for size in sizes {
            let case = format!("{label} at maxResults {size}");
            let pages = pages(
                &emulator,
                &case,
                &json!({ "savingsPlanId": savings_plan_id, "maxResults": size }),
            )
            .await;
            let expected_pages = whole.len().div_ceil(usize::try_from(size).unwrap_or(1));
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages of a {}-line card, got {}",
                whole.len(),
                pages.len()
            );
            for (index, (rows, next)) in pages.iter().enumerate() {
                let last = index + 1 == pages.len();
                assert_eq!(
                    next.is_none(),
                    last,
                    "case {case:?} page {index}: a cursor must come back only while results \
                     remain, got {next:?}"
                );
                assert!(
                    rows.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                    "case {case:?} page {index}: expected at most {size} rows, got {}",
                    rows.len()
                );
                assert!(
                    last || rows.len() == usize::try_from(size).unwrap_or(usize::MAX),
                    "case {case:?} page {index}: a page before the last must be full, got {}",
                    rows.len()
                );
            }
            let walked: Vec<String> = pages
                .into_iter()
                .flat_map(|(rows, _)| rows)
                .map(|rate| key(&rate))
                .collect();
            assert_eq!(
                walked, whole,
                "case {case:?}: paging must yield the card once, in order"
            );
        }

        // A caller that names no page size gets the service's own page, and a
        // cursor whenever the card is longer than it.
        let case = format!("{label} with no maxResults");
        let answer = emulator
            .call(OPERATION, json!({ "savingsPlanId": savings_plan_id }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let page = answer.list("searchResults");
        let expected = whole.len().min(state::DEFAULT_OFFERINGS_PAGE_SIZE);
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: expected the service's page of {expected} line items, got {}",
            page.len()
        );
        assert_eq!(
            page.iter().map(key).collect::<Vec<_>>(),
            whole[..expected],
            "case {case:?}: the service's page must be the first page of the card"
        );
        assert_eq!(
            answer.get("nextToken").as_str().is_some(),
            whole.len() > expected,
            "case {case:?}: expected a cursor only while results remain, got {:?}",
            answer.get("nextToken")
        );
    }
}

/// An id this account never purchased has no rate card: a 404
/// `ResourceNotFoundException` naming the id asked about, never an empty card.
///
/// The rows differ along what the id looks like — an id no purchase minted, the
/// id of a plan's ARN rather than the plan, an offering id, and a word that is
/// no id at all — because the rule is about what the store holds, not about the
/// spelling of the id.
#[tokio::test]
async fn an_unsold_plan_has_no_rate_card() {
    let emulator = Emulator::new();
    let (savings_plan_id, offering, plan) = buy(
        &emulator,
        "SageMaker",
        state::PAYMENT_ALL_UPFRONT,
        ONE_YEAR,
        "0.25",
    )
    .await;

    for (case, unknown) in [
        ("an id no purchase minted", state::savings_plan_arn("x")),
        ("a plan ARN", text(&plan, "savingsPlanArn")),
        ("an offering id", text(&offering, "offeringId")),
        ("no id at all", "not-a-savings-plan".to_string()),
    ] {
        let answer = emulator
            .call(OPERATION, json!({ "savingsPlanId": unknown }))
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for {unknown:?}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.code
        );
        assert!(
            text(&answer.body, "message").contains(&unknown),
            "case {case:?}: the refusal must name the id asked about, got {:?}",
            text(&answer.body, "message")
        );
    }

    // The plan that WAS sold still answers, so the rows above refuse the id and
    // nothing else.
    let answer = emulator
        .call(OPERATION, json!({ "savingsPlanId": savings_plan_id }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case the purchased plan: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// A cursor this search did not issue is refused with a 400
/// `ValidationException`, so a token minted by another paginator cannot resume
/// one caller's walk at another's offset.
#[tokio::test]
async fn a_cursor_from_another_search_is_refused() {
    let emulator = Emulator::new();
    let (savings_plan_id, _, _) = buy(
        &emulator,
        "EC2Instance",
        state::PAYMENT_NO_UPFRONT,
        ONE_YEAR,
        "0.1",
    )
    .await;

    for (case, token) in [
        (
            "a token from the offering-rate search",
            state::encode_page_token("offeringRates", 0),
        ),
        (
            "a token from the plan search",
            state::encode_page_token("plans", 1),
        ),
        ("a token this service never mints", "ZZZZ".to_string()),
    ] {
        let answer = emulator
            .call(
                OPERATION,
                json!({ "savingsPlanId": savings_plan_id, "nextToken": token }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for {token:?}, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            answer.code
        );
    }
}
