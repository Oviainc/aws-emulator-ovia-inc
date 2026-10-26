// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD dimension of a pricing plan itself, end to end over the service's
//! real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: a read scoped to a billing period reports each resource AS IT STOOD in that
//! period — `ListPricingPlans` answers "the billing period for which the described
//! pricing plans are applicable" — so a plan is reported only for the periods from the
//! one it was created in onward, and the members a write can change (`Name`,
//! `Description`, `LastModifiedTime`) carry the values that held in the period asked
//! about rather than the current ones. A write takes effect in the period it is made in,
//! so one period has one generation of the plan and a later write never shows up in an
//! earlier period. A read that names no period asks about the current one.
//!
//! The members no write can change (`Arn` and `CreationTime` — neither carried by
//! `UpdatePricingPlanInput`,
//! `smithy_query(query="shape",shape="UpdatePricingPlanInput")`) are the same in every
//! period, which each table asserts beside the ones that change. `Size` is derived from
//! the plan's rule associations in the period asked about, which
//! `tests/all/pricing_rule_association_periods.rs` owns; this module is about the plan's
//! OWN per-period identity and values.
//!
//! The wire cannot create or update a plan in a PAST period — a write happens now — so a
//! row writes through the real wire and then backdates the stored instants, which is the
//! one thing the test cannot ask the service for.

use serde_json::{json, Value};

use crate::pricing_rule_association_periods::{period_months, shift_period, Emulator};

/// `ListPricingPlans` — the period-scoped read under test.
const LIST_PATH: &str = "/list-pricing-plans";

/// `UpdatePricingPlan`, at the method its modelled HTTP binding names
/// (`smithy_query(query="operation",operation="UpdatePricingPlan")`: a `PUT`).
const UPDATE_PATH: &str = "/update-pricing-plan";

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// Update a pricing plan through the wire.
pub(crate) async fn update_plan(em: &Emulator, case: &str, plan: &str, changes: &Value) {
    let mut body = changes.clone();
    body["Arn"] = json!(plan);
    let (status, answer) = em.put(UPDATE_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: setup UpdatePricingPlan expected 200, got {status} with {answer:?}"
    );
}

/// The `PricingPlanListElement` `ListPricingPlans` reports for ONE plan in `period`
/// (`None` names no period at all), or `None` when the listing does not report the plan
/// for that period at all.
pub(crate) async fn element_in(
    em: &Emulator,
    case: &str,
    plan: &str,
    period: Option<&str>,
) -> Option<Value> {
    let mut body = json!({"Filters": {"Arns": [plan]}});
    if let Some(period) = period {
        body["BillingPeriod"] = json!(period);
    }
    let (status, answer) = em.post(LIST_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListPricingPlans expected 200, got {status} with {answer:?}"
    );
    answer["PricingPlans"]
        .as_array()
        .and_then(|plans| plans.first().cloned())
}

/// Assert that `element` reports every member `expected` names with the value it names.
fn assert_members(case: &str, element: &Value, expected: &Value) {
    let members = expected
        .as_object()
        .expect("an expectation is a map of member to value");
    for (member, value) in members {
        assert_eq!(
            element.get(member).unwrap_or(&Value::Null),
            value,
            "case {case}: expected {member} to be {value}, got {:?} in {element:?}",
            element.get(member)
        );
    }
}

/// The periods a plan created in `creation` must and must not be reported for: the
/// creation period itself and every later one, against the earlier ones. The rows cross
/// a year boundary in both directions.
fn read_periods(creation: &str) -> Vec<(String, bool)> {
    vec![
        (shift_period(creation, -13), false),
        (shift_period(creation, -1), false),
        (creation.to_string(), true),
        (shift_period(creation, 1), true),
        (shift_period(creation, 13), true),
    ]
}

/// The periods a plan of the table is created in. Rows differ along the month form the
/// modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows (`0?[1-9]`, so
/// `2024-1` and `2024-01` are ONE period), the end-of-year and start-of-year months
/// whose neighbours cross a year, the epoch month, and a far-future one.
const CREATION_PERIODS: &[&str] = &["1970-01", "1999-12", "2024-1", "2024-07", "2031-3", "9980-12"];

#[tokio::test]
async fn a_plan_is_reported_only_for_the_periods_from_the_one_it_was_created_in_onward() {
    let em = Emulator::new();
    for (index, creation) in CREATION_PERIODS.iter().enumerate() {
        let case = format!("created in {creation:?}");
        let plan = em.create_plan(&format!("Created-{index}"), &[]).await;
        em.create_plan_in(&plan, creation);

        for (period, exists) in read_periods(creation) {
            let case = format!("created in {creation:?} read at {period:?}");
            let reported = element_in(&em, &case, &plan, Some(&period)).await;
            assert_eq!(
                reported.is_some(),
                exists,
                "case {case}: a plan is reported exactly for the periods from its creation \
                 onward, expected {}, got {reported:?}",
                if exists { "reported" } else { "not reported" }
            );
        }

        // Both spellings of ONE month name one period, so they answer the same.
        let (year, month) = creation
            .split_once('-')
            .expect("a creation period is spelled YYYY-MM");
        let month: u32 = month
            .parse()
            .expect("a creation period has a numeric month");
        for spelling in [format!("{year}-{month}"), format!("{year}-{month:02}")] {
            let case = format!("created in {creation:?} read at {spelling:?}");
            let reported = element_in(&em, &case, &plan, Some(&spelling)).await;
            assert!(
                reported.is_some(),
                "case {case}: either spelling of the creation period reports the plan, got \
                 {reported:?}"
            );
        }

        // A read that names no period asks about the CURRENT one, so a plan created in a
        // period still to come is not reported then either.
        let now = current_period();
        let expected = period_months(creation) <= period_months(&now);
        let reported = element_in(&em, &case, &plan, None).await;
        assert_eq!(
            reported.is_some(),
            expected,
            "case {case} read with no period: a read naming none asks about {now:?}, expected \
             {}, got {reported:?}",
            if expected { "reported" } else { "not reported" }
        );
    }
}

/// One row of the per-period value table: what the plan was created as, what a later
/// write changed, and the members a read of each period must report.
struct ValueRow {
    label: &'static str,
    /// `CreatePricingPlan` members beside `Name`.
    create: Value,
    /// `UpdatePricingPlan` members beside `Arn`.
    update: Value,
    /// The members the earlier period must report — the plan as it was created.
    before: Value,
    /// The members the current period must report — the plan as the write left it.
    after: Value,
}

/// The table: one row per member of `PricingPlanListElement` a write can change
/// (`smithy_query(query="shape",shape="UpdatePricingPlanInput")` carries `Name` and
/// `Description`), each changed by a write that lands in a LATER billing period than the
/// one the plan was created in, plus a row changing both at once.
fn value_rows() -> Vec<ValueRow> {
    vec![
        ValueRow {
            label: "Description",
            create: json!({"Description": "as created"}),
            update: json!({"Description": "as updated"}),
            before: json!({"Description": "as created"}),
            after: json!({"Description": "as updated"}),
        },
        ValueRow {
            label: "Name",
            create: json!({"Description": "unchanged"}),
            update: json!({"Name": "Renamed"}),
            before: json!({"Name": "Valued-1", "Description": "unchanged"}),
            after: json!({"Name": "Renamed", "Description": "unchanged"}),
        },
        ValueRow {
            label: "Name and Description at once",
            create: json!({"Description": "first"}),
            update: json!({"Name": "Renamed_Again", "Description": "last"}),
            before: json!({"Name": "Valued-2", "Description": "first"}),
            after: json!({"Name": "Renamed_Again", "Description": "last"}),
        },
        ValueRow {
            label: "a description given to a plan created without one",
            create: json!({}),
            update: json!({"Description": "added later"}),
            before: json!({"Name": "Valued-3"}),
            after: json!({"Name": "Valued-3", "Description": "added later"}),
        },
        ValueRow {
            label: "the empty description a length of 0 admits",
            create: json!({"Description": "before the empty one"}),
            update: json!({"Description": ""}),
            before: json!({"Description": "before the empty one"}),
            after: json!({"Description": ""}),
        },
    ]
}

#[tokio::test]
async fn a_write_shows_up_from_the_period_it_was_made_in_and_in_no_earlier_one() {
    let em = Emulator::new();
    let now = current_period();
    // The plan is created two years back, so the update the wire makes NOW lands in a
    // later billing period than the creation and therefore opens a new generation.
    let creation = shift_period(&now, -24);

    for (index, row) in value_rows().iter().enumerate() {
        let case = format!("{} rewritten", row.label);
        let name = format!("Valued-{index}");
        let mut create = row.create.clone();
        create["Name"] = json!(name);
        let plan = create_plan_with(&em, &case, &create).await;
        em.create_plan_in(&plan, &creation);
        update_plan(&em, &case, &plan, &row.update).await;

        // The period the plan was created in reports the plan AS CREATED, and so does
        // every period up to the one the write landed in.
        for period in [creation.clone(), shift_period(&now, -1)] {
            let case = format!("{case} read at {period:?}");
            let element = element_in(&em, &case, &plan, Some(&period))
                .await
                .unwrap_or_else(|| panic!("case {case}: the plan existed then, so it is reported"));
            assert_members(&case, &element, &row.before);
            // Nothing a write cannot change moves with the period.
            assert_eq!(
                element["Arn"].as_str(),
                Some(plan.as_str()),
                "case {case}: expected the plan's own ARN, got {element:?}"
            );
            assert_eq!(
                element["Size"].as_i64().unwrap_or_default(),
                0,
                "case {case}: the plan associates no rule, so Size is 0, got {element:?}"
            );
        }

        // The current period reports the plan as the write left it, whether the read
        // names that period or none at all.
        for period in [None, Some(now.clone())] {
            let case = format!("{case} read at {period:?}");
            let element = element_in(&em, &case, &plan, period.as_deref())
                .await
                .unwrap_or_else(|| panic!("case {case}: the plan exists now, so it is reported"));
            assert_members(&case, &element, &row.after);
        }

        // The reads changed nothing: the earlier period still reports the plan as it was
        // created.
        let case = format!("{case} re-read at {creation:?}");
        let element = element_in(&em, &case, &plan, Some(&creation))
            .await
            .unwrap_or_else(|| {
                panic!("case {case}: a read must not change what a later caller sees")
            });
        assert_members(&case, &element, &row.before);
    }
}

#[tokio::test]
async fn each_period_reports_the_generation_that_was_in_force_in_it() {
    let em = Emulator::new();
    let now = current_period();
    // Three generations: the create, a write a year later, and a write made NOW. The
    // first two are backdated, because the wire can only write in the current period.
    let first = shift_period(&now, -24);
    let second = shift_period(&now, -12);

    let plan = create_plan_with(
        &em,
        "three generations",
        &json!({"Name": "Generations", "Description": "first"}),
    )
    .await;
    em.create_plan_in(&plan, &first);
    update_plan(
        &em,
        "three generations",
        &plan,
        &json!({"Name": "Generations_Second", "Description": "second"}),
    )
    .await;
    em.restate_plan_generation_in(&plan, &second);
    update_plan(
        &em,
        "three generations",
        &plan,
        &json!({"Name": "Generations_Third", "Description": "third"}),
    )
    .await;

    // Rows differ along where the period falls among the generations: before the plan
    // existed, the first generation's own period and the month after it, the second
    // generation's period and the month after it, the current period, and a far-future
    // one.
    let rows: Vec<(String, Option<Value>)> = vec![
        (shift_period(&first, -1), None),
        (
            first.clone(),
            Some(json!({"Name": "Generations", "Description": "first"})),
        ),
        (
            shift_period(&first, 1),
            Some(json!({"Name": "Generations", "Description": "first"})),
        ),
        (
            second.clone(),
            Some(json!({"Name": "Generations_Second", "Description": "second"})),
        ),
        (
            shift_period(&second, 1),
            Some(json!({"Name": "Generations_Second", "Description": "second"})),
        ),
        (
            now.clone(),
            Some(json!({"Name": "Generations_Third", "Description": "third"})),
        ),
        (
            shift_period(&now, 13),
            Some(json!({"Name": "Generations_Third", "Description": "third"})),
        ),
    ];

    for (period, expected) in rows {
        let case = format!("generations read at {period:?}");
        let reported = element_in(&em, &case, &plan, Some(&period)).await;
        match (&expected, &reported) {
            (Some(expected), Some(element)) => {
                assert_members(&case, element, expected);
                // `LastModifiedTime` is the generation's own: the instant of the write
                // that left it, so it never moves backwards as the periods advance.
                assert!(
                    element["LastModifiedTime"].as_i64().is_some(),
                    "case {case}: expected the generation's LastModifiedTime, got {element:?}"
                );
            }
            (None, None) => {}
            _ => panic!("case {case}: expected {expected:?}, got {reported:?}"),
        }
    }

    // `LastModifiedTime` orders with the generations: each generation reports the write
    // that left it, so the earlier period reports the earlier instant.
    let mut last_modified: Vec<i64> = Vec::new();
    for period in [&first, &second, &now] {
        let case = format!("LastModifiedTime at {period:?}");
        let element = element_in(&em, &case, &plan, Some(period))
            .await
            .unwrap_or_else(|| panic!("case {case}: the plan existed then, so it is reported"));
        last_modified.push(
            element["LastModifiedTime"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {element:?}")),
        );
        // `CreationTime` is the plan's own and belongs to every generation of it.
        assert_eq!(
            element["CreationTime"].as_i64(),
            Some(last_modified[0]),
            "case {case}: expected the plan's creation instant in every period, got {element:?}"
        );
    }
    assert!(
        last_modified.windows(2).all(|pair| pair[0] < pair[1]),
        "expected each generation's LastModifiedTime to come after the one it replaced, got \
         {last_modified:?}"
    );
}

#[tokio::test]
async fn a_page_walks_only_the_plans_the_period_holds() {
    let em = Emulator::new();
    let now = current_period();
    // Three plans, each created one year apart, so each period holds a different number
    // of them.
    let periods = [
        shift_period(&now, -36),
        shift_period(&now, -24),
        shift_period(&now, -12),
    ];
    let mut plans: Vec<String> = Vec::new();
    for (index, period) in periods.iter().enumerate() {
        let plan = em.create_plan(&format!("Paged-{index}"), &[]).await;
        em.create_plan_in(&plan, period);
        plans.push(plan);
    }

    // Rows differ along how many plans the period holds (0, 1, 2, 3) and along the page
    // size the walk uses, including each bound of the modelled 1..100 `MaxResults` range.
    for (held, period) in [
        (0, shift_period(&periods[0], -1)),
        (1, periods[0].clone()),
        (2, periods[1].clone()),
        (3, periods[2].clone()),
        (3, now.clone()),
    ] {
        let expected: Vec<String> = plans.iter().take(held).cloned().collect();
        for max_results in [1_i64, 2, 3, 100] {
            let case = format!("read at {period:?} with MaxResults={max_results}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({"BillingPeriod": period, "MaxResults": max_results});
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let (status, answer) = em.post(LIST_PATH, &body).await;
                assert_eq!(
                    status, 200,
                    "case {case}: ListPricingPlans expected 200, got {status} with {answer:?}"
                );
                let page = answer["PricingPlans"].as_array().cloned().unwrap_or_default();
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                    "case {case}: a page is at most MaxResults, got {page:?}"
                );
                collected.extend(
                    page.iter()
                        .map(|plan| plan["Arn"].as_str().unwrap_or_default().to_string()),
                );
                token = answer["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case}: a period reports exactly the plans that existed in it, expected \
                 {expected:?}, got {collected:?}"
            );
        }
    }
}

/// `CreatePricingPlan` with whatever members `body` carries, answering the ARN the
/// service minted. The shared harness's own `create_plan` names the associations
/// instead, which these rows do not need.
async fn create_plan_with(em: &Emulator, case: &str, body: &Value) -> String {
    let (status, answer) = em.post("/create-pricing-plan", body).await;
    assert_eq!(
        status, 200,
        "case {case}: setup CreatePricingPlan expected 200, got {status} with {answer:?}"
    );
    answer["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: setup CreatePricingPlan reported no Arn"))
        .to_string()
}
