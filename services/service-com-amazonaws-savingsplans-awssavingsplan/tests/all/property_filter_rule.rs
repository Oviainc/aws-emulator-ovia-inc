// SPDX-License-Identifier: CC0-1.0

//! The ONE admission rule every search that narrows by a named property
//! follows, driven end to end through each search that applies it.
//!
//! The rule: a filter that names no value at all constrains nothing, a row that
//! is not dimensioned by the property a filter names cannot satisfy it, and
//! otherwise the filter admits exactly the rows whose value for that property is
//! one the filter names.
//!
//! The crate holds that rule in one place, and both catalog searches reach it
//! through the type they filter — the offering search through
//! `Offering::matches_filter` and the offering-rate search through
//! `RateLineItem::matches_filter` — so this suite drives the shared path through
//! BOTH searches. Each row's expected answer is computed from the UNFILTERED
//! answer by the rule itself, so a row pins "the filter admits exactly these",
//! not a count someone wrote down.
//!
//! The rows span every filter attribute the model declares for each search
//! (`SavingsPlanOfferingFilterAttribute` and `SavingsPlanRateFilterAttribute`,
//! taken from the generated value sets rather than written out here) and, for
//! each, the forms a filter on one property can take: no `values` member at all,
//! an empty one, one value the answer carries, several, a value nothing carries,
//! and a mix of the two. Each search is driven over a set whose rows are
//! dimensioned DIFFERENTLY — some carry the property, some do not — so both arms
//! of the rule are exercised by the same searches.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, so
//! what is pinned is the HTTP behaviour. The harness (`Emulator`, `text`) is the
//! one the sibling lifecycle suite defines.

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::ops::types::{
    SAVINGS_PLAN_OFFERING_FILTER_ATTRIBUTE_VALUES, SAVINGS_PLAN_RATE_FILTER_ATTRIBUTE_VALUES,
};

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};

/// The offering search.
const OFFERINGS: &str = "DescribeSavingsPlansOfferings";

/// The offering-rate search.
const OFFERING_RATES: &str = "DescribeSavingsPlansOfferingRates";

/// The largest page either search admits (`PageSize` tops out at 1000), used to
/// walk a whole answer in as few calls as possible.
const LARGEST_PAGE: i64 = 1000;

/// A value no row of either catalog carries for any filter attribute: no
/// region, instance family, instance type, product description or tenancy is
/// spelled like this.
const NO_SUCH_VALUE: &str = "no-such-value";

/// One search that narrows by a named property: the search, the request that
/// narrows it to a set small enough to compare whole, and the filter attributes
/// the model declares for it.
struct Subject {
    case: String,
    operation: &'static str,
    base: Value,
    attributes: &'static [&'static str],
}

/// The value a row reports for the named property, or `None` when the row is
/// not dimensioned by it.
///
/// Both wire shapes report their dimensions the same way — a `properties` list
/// of `(name, value)` pairs — so one accessor serves both searches.
fn property(row: &Value, name: &str) -> Option<String> {
    row.get("properties")
        .and_then(Value::as_array)
        .and_then(|properties| {
            properties
                .iter()
                .find(|property| text(property, "name") == name)
        })
        .map(|property| text(property, "value"))
}

/// The rule under test, as one sentence of code: what a filter naming `name`
/// with `values` admits.
fn admits(row: &Value, name: &str, values: Option<&Vec<String>>) -> bool {
    match (values, property(row, name)) {
        (None, _) => true,
        (Some(values), _) if values.is_empty() => true,
        (Some(_), None) => false,
        (Some(values), Some(field)) => values.contains(&field),
    }
}

/// What distinguishes one row of an answer from another: the offering it names
/// (an offering row carries its own id, a rate row its parent's) and the usage
/// it prices.
fn key(row: &Value) -> String {
    let parent = row
        .get("savingsPlanOffering")
        .map_or_else(|| text(row, "offeringId"), |parent| text(parent, "offeringId"));
    format!(
        "{parent}|{}|{}|{}",
        text(row, "productType"),
        text(row, "serviceCode"),
        text(row, "usageType")
    )
}

/// Every row a search admits, walked through its own cursor in pages of
/// [`LARGEST_PAGE`].
async fn walk(emulator: &Emulator, case: &str, operation: &str, request: &Value) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut page_request = request.clone();
        page_request["maxResults"] = json!(LARGEST_PAGE);
        if let Some(token) = &next {
            page_request["nextToken"] = json!(token);
        }
        let answer = emulator.call(operation, page_request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {page_request}, got {} {:?}",
            answer.status, answer.body
        );
        rows.extend(answer.list("searchResults").iter().cloned());
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        pages += 1;
        assert!(
            pages <= 100,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
        if next.is_none() {
            return rows;
        }
    }
}

/// The id of an offering with this plan type, payment option and term.
async fn offering_id(emulator: &Emulator, plan_type: &str, payment_option: &str, term: i64) -> String {
    let offering = emulator.offering(plan_type, payment_option, term).await;
    text(&offering, "offeringId")
}

/// The searches this suite drives, each narrowed to a set whose rows are
/// dimensioned differently from one another.
async fn subjects(emulator: &Emulator) -> Vec<Subject> {
    // Two EC2 Instance offerings, each scoped to its own region and instance
    // family: the rates they price carry every rate dimension but `productId`,
    // and carry two different values of each.
    let scoped: Vec<String> = emulator
        .search_offerings(&json!({
            "planTypes": ["EC2Instance"],
            "paymentOptions": ["No Upfront"],
            "durations": [ONE_YEAR],
        }))
        .await
        .iter()
        .map(|offering| text(offering, "offeringId"))
        .collect();
    assert!(
        scoped.len() >= 2,
        "the catalog sells EC2 Instance offerings in more than one scope, got {}",
        scoped.len()
    );
    let two_scopes = vec![
        scoped.first().cloned().unwrap_or_default(),
        scoped.last().cloned().unwrap_or_default(),
    ];

    // A Compute offering prices Lambda usage, which is dimensioned by region
    // alone; a SageMaker offering prices ML instance usage, which is also
    // dimensioned by instance family and type. Searched together, one answer
    // carries rows of both kinds.
    let compute = offering_id(emulator, "Compute", "All Upfront", ONE_YEAR).await;
    let sagemaker = offering_id(emulator, "SageMaker", "All Upfront", ONE_YEAR).await;

    vec![
        Subject {
            case: "offering rates of two scoped EC2 Instance offerings".to_string(),
            operation: OFFERING_RATES,
            base: json!({
                "savingsPlanOfferingIds": two_scopes,
                "products": ["EC2"],
                "serviceCodes": ["AmazonEC2"],
            }),
            attributes: SAVINGS_PLAN_RATE_FILTER_ATTRIBUTE_VALUES,
        },
        Subject {
            case: "offering rates of usage dimensioned by region alone beside usage that is not"
                .to_string(),
            operation: OFFERING_RATES,
            base: json!({
                "savingsPlanOfferingIds": [compute, sagemaker],
                "products": ["Lambda", "SageMaker"],
            }),
            attributes: SAVINGS_PLAN_RATE_FILTER_ATTRIBUTE_VALUES,
        },
        Subject {
            case: "offerings scoped to a region beside one scoped to none".to_string(),
            operation: OFFERINGS,
            base: json!({
                "planTypes": ["Compute", "SageMaker"],
                "paymentOptions": ["All Upfront"],
                "durations": [ONE_YEAR],
            }),
            attributes: SAVINGS_PLAN_OFFERING_FILTER_ATTRIBUTE_VALUES,
        },
        Subject {
            case: "offerings scoped to a region and an instance family".to_string(),
            operation: OFFERINGS,
            base: json!({
                "planTypes": ["EC2Instance"],
                "paymentOptions": ["Partial Upfront"],
                "durations": [THREE_YEARS],
            }),
            attributes: SAVINGS_PLAN_OFFERING_FILTER_ATTRIBUTE_VALUES,
        },
    ]
}

/// The forms a filter on one property can take, over the values the unfiltered
/// answer carries for it: no `values` member, an empty one, a value nothing
/// carries, one the answer carries, that one mixed with a value nothing carries,
/// a pair, and every value at once.
fn forms(carried: &[String]) -> Vec<Option<Vec<String>>> {
    let mut forms: Vec<Option<Vec<String>>> = vec![
        None,
        Some(Vec::new()),
        Some(vec![NO_SUCH_VALUE.to_string()]),
    ];
    if let Some(first) = carried.first() {
        forms.push(Some(vec![first.clone()]));
        forms.push(Some(vec![first.clone(), NO_SUCH_VALUE.to_string()]));
    }
    if carried.len() >= 2 {
        forms.push(Some(carried[..2].to_vec()));
        forms.push(Some(carried.to_vec()));
    }
    forms
}

/// Every search that narrows by a named property admits exactly the rows the one
/// property-filter rule admits.
#[tokio::test]
async fn one_property_filter_rule_governs_every_catalog_search() {
    let emulator = Emulator::new();

    for subject in subjects(&emulator).await {
        let baseline = walk(&emulator, &subject.case, subject.operation, &subject.base).await;
        assert!(
            !baseline.is_empty(),
            "case {:?}: the catalog carries these rows, so the unfiltered search must answer with them, got none",
            subject.case
        );

        for attribute in subject.attributes {
            let mut carried: Vec<String> = baseline
                .iter()
                .filter_map(|row| property(row, attribute))
                .collect();
            carried.sort();
            carried.dedup();

            for values in forms(&carried) {
                let case = format!("{} filter {attribute}={values:?}", subject.case);
                let expected: Vec<String> = baseline
                    .iter()
                    .filter(|row| admits(row, attribute, values.as_ref()))
                    .map(key)
                    .collect();
                // A filter naming a value the answer does carry must admit
                // something, so no row of this table can pass vacuously.
                if values
                    .as_ref()
                    .is_some_and(|values| values.iter().any(|value| carried.contains(value)))
                {
                    assert!(
                        !expected.is_empty(),
                        "case {case:?}: the answer carries this value, so the filter must admit a row, got none"
                    );
                }

                let mut filter = json!({ "name": attribute });
                if let Some(values) = &values {
                    filter["values"] = json!(values);
                }
                let mut request = subject.base.clone();
                request["filters"] = json!([filter]);
                let found: Vec<String> = walk(&emulator, &case, subject.operation, &request)
                    .await
                    .iter()
                    .map(key)
                    .collect();
                assert_eq!(
                    found, expected,
                    "case {case:?}: expected the {} rows the filter admits, got {}",
                    expected.len(),
                    found.len()
                );
            }
        }
    }
}
