// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD dimension of a pricing rule itself, end to end over the service's
//! real `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE: a read scoped to a billing period reports each resource AS IT STOOD in that
//! period — `ListPricingRules` answers "the billing period for which the described
//! pricing rules are applicable" — so a rule is reported only for the periods from the
//! one it was created in onward, and the members a write can change (`Name`,
//! `Description`, `Type`, `ModifierPercentage`, `Tiering`, `LastModifiedTime`) carry the
//! values that held in the period asked about rather than the current ones. A write
//! takes effect in the period it is made in, so one period has one generation of the
//! rule and a later write never shows up in an earlier period. A read that names no
//! period asks about the current one.
//!
//! The members no write can change (`Arn`, `Scope`, `Service`, `BillingEntity`,
//! `UsageType`, `Operation`, `CreationTime` — none of them carried by
//! `UpdatePricingRuleInput`, `smithy_query(query="shape",shape="UpdatePricingRuleInput")`)
//! are the same in every period, which each table asserts beside the ones that change.
//!
//! `tests/all/pricing_rule_association_periods.rs` owns the same rule for a rule/plan
//! ASSOCIATION and the derived `AssociatedPricingPlanCount`, and owns the harness this
//! suite drives; this module is about the rule's OWN per-period identity and values.
//!
//! The wire cannot create or update a rule in a PAST period — a write happens now — so a
//! row writes through the real wire and then backdates the stored instants, which is the
//! one thing the test cannot ask the service for.

use serde_json::{json, Value};

use crate::pricing_rule_association_periods::{period_months, shift_period, Emulator};

/// `ListPricingRules` — the period-scoped read under test.
const LIST_PATH: &str = "/list-pricing-rules";

/// `CreatePricingRule` / `UpdatePricingRule`, at the methods their modelled HTTP
/// bindings name (`smithy_query(query="operation",operation="UpdatePricingRule")`: a
/// `PUT`).
const CREATE_PATH: &str = "/create-pricing-rule";
const UPDATE_PATH: &str = "/update-pricing-rule";

/// The billing period the service calls current, read from the same clock it answers
/// from so an expectation holds whatever month the suite runs in.
fn current_period() -> String {
    service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period()
}

/// Create a pricing rule through the wire and answer the ARN the service minted.
async fn create_rule(em: &Emulator, case: &str, body: &Value) -> String {
    let (status, answer) = em.post(CREATE_PATH, body).await;
    assert_eq!(
        status, 200,
        "case {case}: setup CreatePricingRule expected 200, got {status} with {answer:?}"
    );
    answer["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: setup CreatePricingRule reported no Arn"))
        .to_string()
}

/// Update a pricing rule through the wire.
async fn update_rule(em: &Emulator, case: &str, rule: &str, changes: &Value) {
    let mut body = changes.clone();
    body["Arn"] = json!(rule);
    let (status, answer) = em.put(UPDATE_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: setup UpdatePricingRule expected 200, got {status} with {answer:?}"
    );
}

/// The `PricingRuleListElement` `ListPricingRules` reports for ONE rule in `period`
/// (`None` names no period at all), or `None` when the listing does not report the rule
/// for that period at all.
async fn element_in(em: &Emulator, case: &str, rule: &str, period: Option<&str>) -> Option<Value> {
    let mut body = json!({"Filters": {"Arns": [rule]}});
    if let Some(period) = period {
        body["BillingPeriod"] = json!(period);
    }
    let (status, answer) = em.post(LIST_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case}: ListPricingRules expected 200, got {status} with {answer:?}"
    );
    answer["PricingRules"]
        .as_array()
        .and_then(|rules| rules.first().cloned())
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

/// The periods a rule created in `creation` must and must not be reported for: the
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

/// The periods a rule of the table is created in. Rows differ along the month form the
/// modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows (`0?[1-9]`, so
/// `2024-1` and `2024-01` are ONE period), the end-of-year and start-of-year months
/// whose neighbours cross a year, the epoch month, and a far-future one.
const CREATION_PERIODS: &[&str] = &["1970-01", "1999-12", "2024-1", "2024-07", "2031-3", "9980-12"];

#[tokio::test]
async fn a_rule_is_reported_only_for_the_periods_from_the_one_it_was_created_in_onward() {
    let em = Emulator::new();
    for (index, creation) in CREATION_PERIODS.iter().enumerate() {
        let case = format!("created in {creation:?}");
        let rule = create_rule(
            &em,
            &case,
            &json!({"Name": format!("Created-{index}"), "Scope": "GLOBAL",
                    "Type": "MARKUP", "ModifierPercentage": 10.0}),
        )
        .await;
        em.create_rule_in(&rule, creation);

        for (period, exists) in read_periods(creation) {
            let case = format!("created in {creation:?} read at {period:?}");
            let reported = element_in(&em, &case, &rule, Some(&period)).await;
            assert_eq!(
                reported.is_some(),
                exists,
                "case {case}: a rule is reported exactly for the periods from its creation \
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
            let reported = element_in(&em, &case, &rule, Some(&spelling)).await;
            assert!(
                reported.is_some(),
                "case {case}: either spelling of the creation period reports the rule, got \
                 {reported:?}"
            );
        }

        // A read that names no period asks about the CURRENT one, so a rule created in a
        // period still to come is not reported then either.
        let case = format!("created in {creation:?} read with no period");
        let now = current_period();
        let expected = period_months(creation) <= period_months(&now);
        let reported = element_in(&em, &case, &rule, None).await;
        assert_eq!(
            reported.is_some(),
            expected,
            "case {case}: a read naming no period asks about {now:?}, expected {}, got \
             {reported:?}",
            if expected { "reported" } else { "not reported" }
        );
    }
}

/// One row of the per-period value table: what the rule was created as, what a later
/// write changed, and the members a read of each period must report.
struct ValueRow {
    label: &'static str,
    /// `CreatePricingRule` members beside `Name`.
    create: Value,
    /// `UpdatePricingRule` members beside `Arn`.
    update: Value,
    /// The members the earlier period must report — the rule as it was created.
    before: Value,
    /// The members the current period must report — the rule as the write left it.
    after: Value,
}

/// The table: one row per member of `PricingRuleListElement` a write can change
/// (`smithy_query(query="shape",shape="UpdatePricingRuleInput")` carries `Name`,
/// `Description`, `Type`, `ModifierPercentage` and `Tiering`), each changed by a write
/// that lands in a LATER billing period than the one the rule was created in.
fn value_rows() -> Vec<ValueRow> {
    vec![
        ValueRow {
            label: "ModifierPercentage",
            create: json!({"Scope": "GLOBAL", "Type": "MARKUP", "ModifierPercentage": 10.0}),
            update: json!({"ModifierPercentage": 20.5}),
            before: json!({"ModifierPercentage": 10.0, "Type": "MARKUP"}),
            after: json!({"ModifierPercentage": 20.5, "Type": "MARKUP"}),
        },
        ValueRow {
            label: "Type",
            create: json!({"Scope": "GLOBAL", "Type": "MARKUP", "ModifierPercentage": 7.0}),
            update: json!({"Type": "DISCOUNT"}),
            before: json!({"Type": "MARKUP", "ModifierPercentage": 7.0}),
            after: json!({"Type": "DISCOUNT", "ModifierPercentage": 7.0}),
        },
        ValueRow {
            label: "Description",
            create: json!({"Scope": "GLOBAL", "Type": "MARKUP", "ModifierPercentage": 1.0,
                           "Description": "as created"}),
            update: json!({"Description": "as updated"}),
            before: json!({"Description": "as created"}),
            after: json!({"Description": "as updated"}),
        },
        ValueRow {
            label: "Tiering",
            create: json!({"Scope": "GLOBAL", "Type": "TIERING",
                           "Tiering": {"FreeTier": {"Activated": true}}}),
            update: json!({"Tiering": {"FreeTier": {"Activated": false}}}),
            before: json!({"Tiering": {"FreeTier": {"Activated": true}}, "Type": "TIERING"}),
            after: json!({"Tiering": {"FreeTier": {"Activated": false}}, "Type": "TIERING"}),
        },
        ValueRow {
            label: "Service and Scope stay put while the rate changes",
            create: json!({"Scope": "SERVICE", "Type": "DISCOUNT", "ModifierPercentage": 15.0,
                           "Service": "AmazonEC2"}),
            update: json!({"ModifierPercentage": 0.0}),
            before: json!({"Scope": "SERVICE", "Service": "AmazonEC2",
                           "ModifierPercentage": 15.0}),
            after: json!({"Scope": "SERVICE", "Service": "AmazonEC2",
                          "ModifierPercentage": 0.0}),
        },
    ]
}

#[tokio::test]
async fn a_write_shows_up_from_the_period_it_was_made_in_and_in_no_earlier_one() {
    let em = Emulator::new();
    let now = current_period();
    // The rule is created two years back, so the update the wire makes NOW lands in a
    // later billing period than the creation and therefore opens a new generation.
    let creation = shift_period(&now, -24);

    for (index, row) in value_rows().iter().enumerate() {
        let case = format!("{} rewritten", row.label);
        let mut create = row.create.clone();
        create["Name"] = json!(format!("Valued-{index}"));
        let rule = create_rule(&em, &case, &create).await;
        em.create_rule_in(&rule, &creation);
        update_rule(&em, &case, &rule, &row.update).await;

        // The period the rule was created in reports the rule AS CREATED, and so does
        // every period up to the one the write landed in.
        for period in [creation.clone(), shift_period(&now, -1)] {
            let case = format!("{case} read at {period:?}");
            let element = element_in(&em, &case, &rule, Some(&period))
                .await
                .unwrap_or_else(|| panic!("case {case}: the rule existed then, so it is reported"));
            assert_members(&case, &element, &row.before);
            // Nothing a write cannot change moves with the period.
            assert_eq!(
                element["Arn"].as_str(),
                Some(rule.as_str()),
                "case {case}: expected the rule's own ARN, got {element:?}"
            );
            assert_eq!(
                element["Name"].as_str(),
                Some(format!("Valued-{index}").as_str()),
                "case {case}: expected the name the create gave, got {element:?}"
            );
        }

        // The current period reports the rule as the write left it, whether the read
        // names that period or none at all.
        for period in [None, Some(now.clone())] {
            let case = format!("{case} read at {period:?}");
            let element = element_in(&em, &case, &rule, period.as_deref())
                .await
                .unwrap_or_else(|| panic!("case {case}: the rule exists now, so it is reported"));
            assert_members(&case, &element, &row.after);
        }

        // The reads changed nothing: the earlier period still reports the rule as it was
        // created.
        let case = format!("{case} re-read at {creation:?}");
        let element = element_in(&em, &case, &rule, Some(&creation))
            .await
            .unwrap_or_else(|| panic!("case {case}: a read must not change what a later caller \
                                       sees"));
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

    let rule = create_rule(
        &em,
        "three generations",
        &json!({"Name": "Generations", "Scope": "GLOBAL", "Type": "MARKUP",
                "ModifierPercentage": 1.0, "Description": "first"}),
    )
    .await;
    em.create_rule_in(&rule, &first);
    update_rule(
        &em,
        "three generations",
        &rule,
        &json!({"ModifierPercentage": 2.0, "Description": "second"}),
    )
    .await;
    em.restate_generation_in(&rule, &second);
    update_rule(
        &em,
        "three generations",
        &rule,
        &json!({"ModifierPercentage": 3.0, "Description": "third"}),
    )
    .await;

    // Rows differ along where the period falls among the generations: before the rule
    // existed, the first generation's own period and the month after it, the second
    // generation's period and the month after it, and the current period.
    let rows: Vec<(String, Option<Value>)> = vec![
        (shift_period(&first, -1), None),
        (
            first.clone(),
            Some(json!({"ModifierPercentage": 1.0, "Description": "first"})),
        ),
        (
            shift_period(&first, 1),
            Some(json!({"ModifierPercentage": 1.0, "Description": "first"})),
        ),
        (
            second.clone(),
            Some(json!({"ModifierPercentage": 2.0, "Description": "second"})),
        ),
        (
            shift_period(&second, 1),
            Some(json!({"ModifierPercentage": 2.0, "Description": "second"})),
        ),
        (
            now.clone(),
            Some(json!({"ModifierPercentage": 3.0, "Description": "third"})),
        ),
        (
            shift_period(&now, 13),
            Some(json!({"ModifierPercentage": 3.0, "Description": "third"})),
        ),
    ];

    for (period, expected) in rows {
        let case = format!("generations read at {period:?}");
        let reported = element_in(&em, &case, &rule, Some(&period)).await;
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
            _ => panic!(
                "case {case}: expected {expected:?}, got {reported:?}"
            ),
        }
    }

    // `LastModifiedTime` orders with the generations: each generation reports the write
    // that left it, so the earlier period reports the earlier instant.
    let mut last_modified: Vec<i64> = Vec::new();
    for period in [&first, &second, &now] {
        let case = format!("LastModifiedTime at {period:?}");
        let element = element_in(&em, &case, &rule, Some(period))
            .await
            .unwrap_or_else(|| panic!("case {case}: the rule existed then, so it is reported"));
        last_modified.push(
            element["LastModifiedTime"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case}: no LastModifiedTime in {element:?}")),
        );
        // `CreationTime` is the rule's own and belongs to every generation of it.
        assert_eq!(
            element["CreationTime"].as_i64(),
            Some(last_modified[0]),
            "case {case}: expected the rule's creation instant in every period, got {element:?}"
        );
    }
    assert!(
        last_modified.windows(2).all(|pair| pair[0] < pair[1]),
        "expected each generation's LastModifiedTime to come after the one it replaced, got \
         {last_modified:?}"
    );
}

#[tokio::test]
async fn a_page_walks_only_the_rules_the_period_holds() {
    let em = Emulator::new();
    let now = current_period();
    // Three rules, each created one year apart, so each period holds a different number
    // of them.
    let periods = [
        shift_period(&now, -36),
        shift_period(&now, -24),
        shift_period(&now, -12),
    ];
    let mut rules: Vec<String> = Vec::new();
    for (index, period) in periods.iter().enumerate() {
        let rule = create_rule(
            &em,
            &format!("paged rule {index}"),
            &json!({"Name": format!("Paged-{index}"), "Scope": "GLOBAL", "Type": "MARKUP",
                    "ModifierPercentage": f64::from(u32::try_from(index).unwrap_or_default())}),
        )
        .await;
        em.create_rule_in(&rule, period);
        rules.push(rule);
    }

    // Rows differ along how many rules the period holds (0, 1, 2, 3) and along the page
    // size the walk uses, including each bound of the modelled 1..100 `MaxResults` range.
    for (held, period) in [
        (0, shift_period(&periods[0], -1)),
        (1, periods[0].clone()),
        (2, periods[1].clone()),
        (3, periods[2].clone()),
        (3, now.clone()),
    ] {
        let expected: Vec<String> = rules.iter().take(held).cloned().collect();
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
                    "case {case}: ListPricingRules expected 200, got {status} with {answer:?}"
                );
                let page = answer["PricingRules"].as_array().cloned().unwrap_or_default();
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= max_results,
                    "case {case}: a page is at most MaxResults, got {page:?}"
                );
                collected.extend(
                    page.iter()
                        .map(|rule| rule["Arn"].as_str().unwrap_or_default().to_string()),
                );
                token = answer["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                collected, expected,
                "case {case}: a period reports exactly the rules that existed in it, expected \
                 {expected:?}, got {collected:?}"
            );
        }
    }
}
