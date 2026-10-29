// SPDX-License-Identifier: CC0-1.0

//! What a `GetBillingGroupCostReport` `NextToken` is a cursor INTO, end to end over the
//! wire.
//!
//! RULE under test: a pagination token is a cursor into ONE sequence, so the scope it
//! carries names every request value that decides WHICH sequence is read — for this report
//! the billing group, the breakdown `GroupBy` asks for (which decides what one row IS and
//! so what its cursor key spells) and the billing periods the range covers (which decide
//! which rows exist at all and what each one's amounts cover) — and a token handed back on
//! a request that names a DIFFERENT sequence is refused with the modelled 400
//! `ValidationException`, `Reason` `CANNOT_PARSE` and `Fields` naming `NextToken`, rather
//! than cutting that other sequence at a key it names no position in. A request that names
//! the SAME sequence, however it spelled it, resumes exactly where the page stopped.
//!
//! The rows span what the rule generalises over rather than one reported call: per minted
//! token, a second request differing in the breakdown (absent, empty, narrowed, widened,
//! swapped for the other attribute), in the range (absent, wider, narrower), or in the
//! parent group — and, on the resuming side, every spelling that names the one sequence
//! (the attributes in another order or named twice, the group by its bare id, the covered
//! period named by the range that enumerates it, a month spelled without its leading
//! zero).
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`], on the shared harness of
//! `tests/all/payer_account_gate.rs` — this service's control plane answers the payer
//! account alone.

use serde_json::{json, Value};

use crate::pagination_token_opacity::period;
use crate::payer_account_gate::{error_code, Emulator};

/// One of the pricing plans Amazon Web Services owns, in the full-ARN form
/// `ComputationPreference.PricingPlanArn`'s `PricingPlanFullArn` pattern requires
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`) — a plan a billing group may
/// name without anything having created it.
const MANAGED_PLAN_ARN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// How many distinct products the fixture presents charges under, and so how many rows a
/// breakdown by product has: ONE past the smallest page this read can be asked for, so a
/// first page of that size always hands back a `NextToken` and leaves exactly one row.
const PRODUCTS: usize = 201;

/// The page every first call here asks for: the minimum of the modelled
/// `MaxBillingGroupCostReportResults` range (200..300,
/// `smithy_query(query="operation",operation="GetBillingGroupCostReport")`).
const PAGE: usize = 200;

/// The route this read is bound to.
const PATH: &str = "/get-billing-group-cost-report";

/// The fixture: one billing group whose report pages, and a second group as another parent.
struct World {
    em: Emulator,
    /// The billing group the report under test is of.
    group: String,
    /// Another billing group — the parent whose own sequence a token of the first names no
    /// position in.
    other_group: String,
}

/// The product the `index`th custom line item presents its charges under, zero-padded so
/// the names sort the way the index does (`PresentationObject.Service` is 1..128 characters
/// matching `^[a-zA-Z0-9]+$`;
/// `smithy_query(query="shape",shape="PresentationObject")`).
fn product(index: usize) -> String {
    format!("Svc{index:03}")
}

/// Send one setup call that must succeed and return the `Arn` it answered with.
async fn setup_arn(em: &Emulator, path: &str, body: &Value, label: &str) -> String {
    let (status, response, _) = em.call("POST", path, Some(body)).await;
    assert_eq!(
        status, 200,
        "setup {label}: expected HTTP 200 from {path}, got {status} with {response}"
    );
    response
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {label}: expected an Arn member, got {response}"))
        .to_string()
}

/// Stand up two billing groups and give the first one a flat fee under each of
/// [`PRODUCTS`] distinct products, so its report has one row per product.
async fn fixture() -> World {
    let em = Emulator::payer();

    let mut groups: Vec<String> = Vec::new();
    for index in 0..2 {
        groups.push(
            setup_arn(
                &em,
                "/create-billing-group",
                &json!({
                    "Name": format!("ScopeGroup{index}"),
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PLAN_ARN },
                }),
                &format!("billing group {index}"),
            )
            .await,
        );
    }

    for index in 0..PRODUCTS {
        setup_arn(
            &em,
            "/create-custom-line-item",
            &json!({
                "Name": format!("ScopeItem{index:03}"),
                "Description": "an item of the cost-report token-scope fixture",
                "BillingGroupArn": groups[0],
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
                "PresentationDetails": { "Service": product(index) },
            }),
            &format!("custom line item {index}"),
        )
        .await;
    }

    World {
        em,
        group: groups[0].clone(),
        other_group: groups[1].clone(),
    }
}

/// `base` with the members `extra` carries written onto it.
fn request(base: &Value, extra: Value) -> Value {
    let mut request = base.clone();
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The rows of a report response, or none when it carries no page.
fn rows(body: &Value) -> Vec<Value> {
    body.get("BillingGroupCostReportResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every row `shape` reports with no page bound of its own — the sequence a token minted
/// over it is a cursor into.
async fn whole_report(em: &Emulator, case: &str, shape: &Value) -> Vec<Value> {
    let (status, body, _) = em.call("POST", PATH, Some(shape)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the unpaged report to be served, got {status} with {body}"
    );
    assert_eq!(
        body.get("NextToken"),
        None,
        "case {case:?}: the server's own page holds every row here, so no NextToken is due; \
         got {body}"
    );
    rows(&body)
}

/// The `NextToken` the smallest page `shape` can be asked for hands back.
async fn minted_token(em: &Emulator, case: &str, shape: &Value) -> String {
    let body = request(shape, json!({ "MaxResults": PAGE }));
    let (status, response, _) = em.call("POST", PATH, Some(&body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a first page, got {status} with {response}"
    );
    let page = rows(&response);
    assert_eq!(
        page.len(),
        PAGE,
        "case {case:?}: expected the {PAGE} rows asked for, got {} rows",
        page.len()
    );
    response
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a NextToken for a {PAGE}-row page of a longer \
                 sequence, got {response}"
            )
        })
        .to_string()
}

/// Assert a report asked to resume from a token minted over ANOTHER sequence is refused as
/// a token this read did not mint for the sequence being read: the modelled 400
/// `ValidationException`, `Reason` `CANNOT_PARSE`, `Fields` naming `NextToken`, and no page
/// at all.
async fn assert_refused(em: &Emulator, case: &str, body: &Value) {
    let (status, response, error_type) = em.call("POST", PATH, Some(body)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a token minted over another sequence, got \
         {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {response}"
    );
    assert_eq!(
        error_type
            .as_deref()
            .map(|value| value.rsplit('#').next().unwrap_or_default().to_string()),
        Some("ValidationException".to_string()),
        "case {case:?}: expected the error shape in x-amzn-errortype, got {error_type:?}"
    );
    assert_eq!(
        response.get("Reason").and_then(Value::as_str),
        Some("CANNOT_PARSE"),
        "case {case:?}: expected Reason CANNOT_PARSE, got {response}"
    );
    let fields: Vec<String> = response
        .get("Fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|field| field.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(
        fields,
        vec!["NextToken".to_string()],
        "case {case:?}: expected Fields to name NextToken, got {response}"
    );
    assert!(
        response.get("BillingGroupCostReportResults").is_none(),
        "case {case:?}: the refusal must carry no page of rows, got {response}"
    );
    assert!(
        response.get("NextToken").is_none(),
        "case {case:?}: the refusal must carry no NextToken, got {response}"
    );
}

/// The bare resource id of a full ARN — the OTHER form the `BillingGroupArn` pattern
/// admits, so a request may name the same group either way.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or(arn).to_string()
}

/// The same billing period with its month spelled without a leading zero, the other
/// spelling the modelled `BillingPeriod` pattern `^\d{4}-(0?[1-9]|1[012])$` allows. A month
/// from October on has only the one spelling.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// THE RULE, refusing side: a token minted under one breakdown, range or parent group names
/// no position in another's sequence, so handing it back on a request that names a different
/// sequence is refused rather than cutting that sequence at a key above all of it.
#[tokio::test]
async fn a_cost_report_token_is_refused_by_a_request_naming_another_sequence() {
    let world = fixture().await;
    let group = world.group.clone();

    // The three breakdown/range shapes this fixture mints a token over: a breakdown by
    // product in the implicit current period, the same plus the billing period, and a
    // breakdown by product over a two-month range.
    let by_product = json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME"] });
    let by_period_and_product =
        json!({ "Arn": group, "GroupBy": ["BILLING_PERIOD", "PRODUCT_NAME"] });
    let over_two_periods = json!({
        "Arn": group,
        "GroupBy": ["PRODUCT_NAME"],
        "BillingPeriodRange": {
            "InclusiveStartBillingPeriod": period(-1),
            "ExclusiveEndBillingPeriod": period(1),
        },
    });

    // (case, the shape the token is minted over, the shape it is handed back on) — one row
    // per way a second request can name a sequence the cursor points into nothing of.
    let cases: Vec<(&str, &Value, Value)> = vec![
        (
            "a product breakdown's token on no breakdown at all",
            &by_product,
            json!({ "Arn": group }),
        ),
        (
            "a product breakdown's token on an empty breakdown",
            &by_product,
            json!({ "Arn": group, "GroupBy": [] }),
        ),
        (
            "a product breakdown's token on the other attribute",
            &by_product,
            json!({ "Arn": group, "GroupBy": ["BILLING_PERIOD"] }),
        ),
        (
            "a product breakdown's token on a finer breakdown",
            &by_product,
            json!({ "Arn": group, "GroupBy": ["BILLING_PERIOD", "PRODUCT_NAME"] }),
        ),
        (
            "a product breakdown's token on a wider range",
            &by_product,
            json!({
                "Arn": group,
                "GroupBy": ["PRODUCT_NAME"],
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(-1),
                    "ExclusiveEndBillingPeriod": period(1),
                },
            }),
        ),
        (
            "a product breakdown's token on another billing group",
            &by_product,
            json!({ "Arn": world.other_group, "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "a two-attribute breakdown's token on a coarser breakdown",
            &by_period_and_product,
            json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "a two-attribute breakdown's token on the other single attribute",
            &by_period_and_product,
            json!({ "Arn": group, "GroupBy": ["BILLING_PERIOD"] }),
        ),
        (
            "a two-month range's token on the implicit current period",
            &over_two_periods,
            json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "a two-month range's token on a narrower range",
            &over_two_periods,
            json!({
                "Arn": group,
                "GroupBy": ["PRODUCT_NAME"],
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(0),
                    "ExclusiveEndBillingPeriod": period(1),
                },
            }),
        ),
    ];

    for (case, minting, second) in cases {
        // The sequence the second request serves on its own, so the refusal can be shown to
        // have changed nothing about it.
        let before = whole_report(&world.em, case, &second).await;
        let token = minted_token(&world.em, case, minting).await;
        assert_refused(
            &world.em,
            case,
            &request(&second, json!({ "NextToken": token })),
        )
        .await;
        let after = whole_report(&world.em, case, &second).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused token must change nothing a later caller reads, got \
             {after:?}"
        );
    }
}

/// THE RULE, resuming side: a request that names the SAME sequence resumes exactly where
/// the page stopped, whatever spelling it names that sequence with.
#[tokio::test]
async fn a_cost_report_token_resumes_every_request_naming_the_sequence_it_was_minted_over() {
    let world = fixture().await;
    let group = world.group.clone();

    let by_product = json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME"] });
    let by_period_and_product =
        json!({ "Arn": group, "GroupBy": ["BILLING_PERIOD", "PRODUCT_NAME"] });
    let over_two_periods = json!({
        "Arn": group,
        "GroupBy": ["PRODUCT_NAME"],
        "BillingPeriodRange": {
            "InclusiveStartBillingPeriod": period(-1),
            "ExclusiveEndBillingPeriod": period(1),
        },
    });

    // (case, the shape the token is minted over, the shape it is handed back on) — the
    // second shape names the SAME sequence as the first in every row, so each resumes it.
    let cases: Vec<(&str, &Value, Value)> = vec![
        (
            "the same breakdown",
            &by_product,
            json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "the same attribute named twice",
            &by_product,
            json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME", "PRODUCT_NAME"] }),
        ),
        (
            "the group named by its bare resource id",
            &by_product,
            json!({ "Arn": bare_id(&group), "GroupBy": ["PRODUCT_NAME"] }),
        ),
        (
            "the implicit current period named as the range that covers it",
            &by_product,
            json!({
                "Arn": group,
                "GroupBy": ["PRODUCT_NAME"],
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": period(0),
                    "ExclusiveEndBillingPeriod": period(1),
                },
            }),
        ),
        (
            "the same two attributes in the other order",
            &by_period_and_product,
            json!({ "Arn": group, "GroupBy": ["PRODUCT_NAME", "BILLING_PERIOD"] }),
        ),
        (
            "the same range with a month spelled without its leading zero",
            &over_two_periods,
            json!({
                "Arn": group,
                "GroupBy": ["PRODUCT_NAME"],
                "BillingPeriodRange": {
                    "InclusiveStartBillingPeriod": one_digit_month(&period(-1)),
                    "ExclusiveEndBillingPeriod": one_digit_month(&period(1)),
                },
            }),
        ),
    ];

    for (case, minting, second) in cases {
        let whole = whole_report(&world.em, case, &second).await;
        assert_eq!(
            whole.len(),
            PRODUCTS,
            "case {case:?}: expected one row per product in the unpaged report, got {} rows",
            whole.len()
        );
        let token = minted_token(&world.em, case, minting).await;
        let resumed = whole_report(
            &world.em,
            case,
            &request(&second, json!({ "NextToken": token })),
        )
        .await;
        assert_eq!(
            resumed,
            whole[PAGE..].to_vec(),
            "case {case:?}: expected the rows the first page left out, got {resumed:?}"
        );
    }
}
