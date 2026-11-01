// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListEfficiencyMetrics`, driven over the wire
//! (`awsJson1_0`) through the same multi-service router the emulator boots.
//!
//! The rule under test: the answer is the recommendation catalog the caller may
//! read, reduced to one recommendation per resource, totalled per value of the
//! `groupBy` dimension — or into one group covering every resource where no
//! dimension is named — and reported as one data point per interval of the
//! requested granularity that the requested period reaches, each point carrying
//! the month's figures prorated by the days of that month it covers inside the
//! period.
//!
//! Every money figure is checked against the SAME rows read through
//! `ListRecommendations` and `ListRecommendationSummaries`, aggregated by the
//! test: the oracle is another view of the store, never a number copied into
//! this file. The tables span both granularities, the date forms the member
//! accepts, months of 28, 29, 30 and 31 days, partially covered months, every
//! grouping dimension, both savings estimation modes, each ordering dimension in
//! both directions, and every page size.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    assert_stated_rule_refusal, call, enrolled_router, enrolled_router_with_discounts_visible, ok,
    router,
};

/// The dimensions cost efficiency metrics can be grouped by, each paired with
/// the `Recommendation` member it names, so a case can total the list view along
/// the same member.
///
/// The names are the model's own (`smithy_query(query="shape",
/// shape="Recommendation")`), with `ResourceType` the name
/// `smithy_query(query="shape", shape="Filter")` gives the type the resource has
/// today. The operation documents "account ID, Amazon Web Services Region"
/// among the valid values; this service groups these metrics by the same
/// categorical dimensions every other aggregated read of it groups by
/// (`state::GROUP_DIMENSIONS`), so the table spans those too.
const GROUPINGS: &[(&str, &str)] = &[
    ("AccountId", "accountId"),
    ("Region", "region"),
    ("ResourceType", "currentResourceType"),
    ("CurrentResourceType", "currentResourceType"),
    ("RecommendedResourceType", "recommendedResourceType"),
    ("ActionType", "actionType"),
    ("ImplementationEffort", "implementationEffort"),
    ("Source", "source"),
    ("CurrencyCode", "currencyCode"),
    ("RestartNeeded", "restartNeeded"),
    ("RollbackPossible", "rollbackPossible"),
    // The dimension is resolved case-insensitively, as every other dimension of
    // this service is, so the SDK spelling and the lower-case one name the same
    // member.
    ("region", "region"),
    ("ACTIONTYPE", "actionType"),
];

/// Money in whole cents, so two figures derived by different routes compare
/// exactly rather than by float identity.
fn cents(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

/// A well-formed request for one granularity over one period.
fn request(granularity: &str, start: &str, end: &str) -> Value {
    json!({
        "granularity": granularity,
        "timePeriod": {"start": start, "end": end},
    })
}

/// The groups one answer reports, in the order it carries them.
fn groups(payload: &Value) -> Vec<Value> {
    payload["efficiencyMetricsByGroup"]
        .as_array()
        .unwrap_or_else(|| panic!("case groups: expected a list of groups in {payload}"))
        .clone()
}

/// The dimension value a group reports, or `None` where it reports none —
/// which is what a request that asked for no grouping gets.
fn label(group: &Value) -> Option<String> {
    group["group"].as_str().map(str::to_string)
}

/// The timestamps one group's series carries, in order.
fn stamps(group: &Value) -> Vec<String> {
    group["metricsByTime"]
        .as_array()
        .unwrap_or_else(|| panic!("case stamps: expected a series in {group}"))
        .iter()
        .map(|point| {
            point["timestamp"]
                .as_str()
                .unwrap_or_else(|| panic!("case stamps: expected a timestamp in {point}"))
                .to_string()
        })
        .collect()
}

/// One group's spend and savings totalled over its whole series, in whole cents.
fn totals(group: &Value) -> (i64, i64) {
    let series = group["metricsByTime"]
        .as_array()
        .unwrap_or_else(|| panic!("case totals: expected a series in {group}"))
        .clone();
    let spend = series
        .iter()
        .map(|point| point["spend"].as_f64().unwrap_or_default())
        .sum::<f64>();
    let savings = series
        .iter()
        .map(|point| point["savings"].as_f64().unwrap_or_default())
        .sum::<f64>();
    (cents(spend), cents(savings))
}

/// The one efficiency score a group reports: the score is a share of the group's
/// spend, so every point of its series carries it.
fn score(group: &Value) -> f64 {
    let series = group["metricsByTime"]
        .as_array()
        .unwrap_or_else(|| panic!("case score: expected a series in {group}"));
    let scores: Vec<f64> = series
        .iter()
        .map(|point| point["score"].as_f64().unwrap_or_default())
        .collect();
    let first = *scores
        .first()
        .unwrap_or_else(|| panic!("case score: expected at least one data point in {group}"));
    assert!(
        scores.iter().all(|score| (*score - first).abs() < f64::EPSILON),
        "case score: expected one score across the series, got {scores:?}"
    );
    first
}

/// The rows every figure of this answer is taken over, as `ListRecommendations`
/// reports them: ONE recommendation per resource, which is the de-duplicated
/// view a resource's mutually exclusive alternatives are reduced to.
async fn deduped(router: &MultiServiceRouter) -> Vec<Value> {
    let payload = ok(
        router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": false}),
    )
    .await;
    payload["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case oracle: expected a list of items in {payload}"))
        .clone()
}

/// The label a recommendation falls under for one member: the member's value as
/// text, whatever type the member carries.
fn member_label(item: &Value, member: &str) -> String {
    match &item[member] {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        other => panic!("case member {member:?}: no label for {other}"),
    }
}

/// What the listed rows must total to, per value of one member — or, where no
/// member is named, over all of them as one group: `(label, spend, savings)` in
/// whole cents, by label, which is the order the answer carries its groups in
/// when the request asks for no other.
fn expected(rows: &[Value], member: Option<&str>) -> Vec<(Option<String>, i64, i64)> {
    let mut totals: Vec<(Option<String>, i64, i64)> = Vec::new();
    for row in rows {
        let label = member.map(|member| member_label(row, member));
        let spend = cents(row["estimatedMonthlyCost"].as_f64().unwrap_or_default());
        let savings = cents(row["estimatedMonthlySavings"].as_f64().unwrap_or_default());
        match totals.iter_mut().find(|(named, _, _)| *named == label) {
            Some((_, group_spend, group_savings)) => {
                *group_spend += spend;
                *group_savings += savings;
            }
            None => totals.push((label, spend, savings)),
        }
    }
    if member.is_none() && totals.is_empty() {
        totals.push((None, 0, 0));
    }
    totals.sort_by(|a, b| a.0.cmp(&b.0));
    totals
}

/// Move the account to one `savingsEstimationMode`, over the wire.
async fn set_mode(router: &MultiServiceRouter, mode: &str) {
    ok(
        router,
        "UpdatePreferences",
        &json!({"savingsEstimationMode": mode}),
    )
    .await;
}

/// A whole month, as the two dates that cover it exactly.
const MARCH: (&str, &str) = ("2026-03-01", "2026-04-01");

// ---------------------------------------------------------------------------
// The time series
// ---------------------------------------------------------------------------

/// One data point per interval of the requested granularity the period reaches,
/// timestamped as that granularity is documented to be (`YYYY-MM-DD` for daily,
/// `YYYY-MM` for monthly) — over a table spanning both granularities, both date
/// forms the member accepts (`YYYY-MM-DD` and `YYYY-MM`), a month boundary, a
/// leap day, and months the period only partly covers.
#[tokio::test]
async fn the_series_carries_one_point_per_interval_the_period_reaches() {
    let router = enrolled_router().await;

    let cases: Vec<(&str, &str, &str, Vec<&str>)> = vec![
        ("Daily", "2026-03-01", "2026-03-02", vec!["2026-03-01"]),
        (
            "Daily",
            "2026-01-30",
            "2026-02-02",
            vec!["2026-01-30", "2026-01-31", "2026-02-01"],
        ),
        // February of a leap year has a 29th, which the series has to carry.
        (
            "Daily",
            "2024-02-28",
            "2024-03-01",
            vec!["2024-02-28", "2024-02-29"],
        ),
        // A month on its own names its first day, so this is the whole of March.
        ("Daily", "2026-03", "2026-03-04", vec![
            "2026-03-01",
            "2026-03-02",
            "2026-03-03",
        ]),
        ("Monthly", "2026-03-01", "2026-04-01", vec!["2026-03"]),
        (
            "Monthly",
            "2025-11-01",
            "2026-02-01",
            vec!["2025-11", "2025-12", "2026-01"],
        ),
        (
            "Monthly",
            "2026-01",
            "2026-04",
            vec!["2026-01", "2026-02", "2026-03"],
        ),
        // Both ends fall inside a month, so both of those months are reached.
        (
            "Monthly",
            "2026-01-15",
            "2026-02-15",
            vec!["2026-01", "2026-02"],
        ),
    ];

    for (granularity, start, end, expected_stamps) in &cases {
        let case = format!("{granularity} from {start} to {end}");
        let payload = ok(
            &router,
            "ListEfficiencyMetrics",
            &request(granularity, start, end),
        )
        .await;
        let answered = groups(&payload);
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: a request with no grouping reports one group, got {answered:?}"
        );
        for group in &answered {
            let carried = stamps(group);
            assert_eq!(
                carried, *expected_stamps,
                "case {case:?}: expected the points {expected_stamps:?}, got {carried:?}"
            );
        }
    }
}

/// A point's figures are the month's prorated by the days of that month it
/// covers inside the period, so the DAILY points of a month add up to that
/// month's MONTHLY point — asserted over months of 28, 29, 30 and 31 days, and
/// over a month the period only half covers, where both granularities report the
/// same covered share.
#[tokio::test]
async fn the_daily_points_of_a_month_add_up_to_its_monthly_point() {
    let router = enrolled_router().await;

    // (case, start, end): each a period the two granularities must agree over.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a 31-day month", "2026-01-01", "2026-02-01"),
        ("a 28-day month", "2026-02-01", "2026-03-01"),
        ("a 29-day month of a leap year", "2024-02-01", "2024-03-01"),
        ("a 30-day month", "2026-04-01", "2026-05-01"),
        ("three whole months", "2026-01-01", "2026-04-01"),
        ("half of a month", "2026-01-01", "2026-01-16"),
        ("the tail of one month and the head of the next", "2026-01-20", "2026-02-10"),
    ];

    for (name, start, end) in &cases {
        let daily = groups(
            &ok(
                &router,
                "ListEfficiencyMetrics",
                &request("Daily", start, end),
            )
            .await,
        );
        let monthly = groups(
            &ok(
                &router,
                "ListEfficiencyMetrics",
                &request("Monthly", start, end),
            )
            .await,
        );
        assert_eq!(
            daily.len(),
            monthly.len(),
            "case {name:?}: expected the same groups at both granularities, got \
             {daily:?} and {monthly:?}"
        );

        for (day_group, month_group) in daily.iter().zip(monthly.iter()) {
            let case = format!("{name} grouped as {:?}", label(day_group));
            let (day_spend, day_savings) = totals(day_group);
            let (month_spend, month_savings) = totals(month_group);
            // Every point is rounded to whole cents, so a sum of N points can
            // differ from the single point covering the same days by at most
            // half a cent per point.
            let slack = stamps(day_group).len() as i64 / 2 + 1;
            assert!(
                (day_spend - month_spend).abs() <= slack,
                "case {case:?}: expected the daily spend {day_spend} to add up to the \
                 monthly spend {month_spend} (within {slack} cents)"
            );
            assert!(
                (day_savings - month_savings).abs() <= slack,
                "case {case:?}: expected the daily savings {day_savings} to add up to the \
                 monthly savings {month_savings} (within {slack} cents)"
            );
            assert!(
                (score(day_group) - score(month_group)).abs() < 0.02,
                "case {case:?}: the score is a share of spend, so it does not depend on the \
                 granularity: got {} daily and {} monthly",
                score(day_group),
                score(month_group)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The figures, and the grouping they are totalled over
// ---------------------------------------------------------------------------

/// Every group's spend and savings are the de-duplicated recommendation rows
/// totalled along the grouping dimension — the same rows `ListRecommendations`
/// reports — and its score is the share of that spend the savings do not cover.
///
/// Spanned over every grouping dimension, the no-grouping case (one group
/// covering all resources), and both savings estimation modes, because the mode
/// decides whether the figures are net of the discounts the resources already
/// carry.
#[tokio::test]
async fn every_group_totals_the_rows_the_recommendation_readers_report() {
    // (case, the router whose account is in that mode)
    let default_mode = enrolled_router().await;
    let discounted = enrolled_router_with_discounts_visible().await;
    set_mode(&discounted, "AfterDiscounts").await;

    for (mode, router) in [
        ("the default mode", &default_mode),
        ("AfterDiscounts", &discounted),
    ] {
        let rows = deduped(router).await;
        assert!(
            !rows.is_empty(),
            "case {mode}: expected the catalog to hold rows to total"
        );

        // The groupings, plus the request that names none at all.
        let mut groupings: Vec<(Option<&str>, Option<&str>)> =
            vec![(None, None)];
        groupings.extend(
            GROUPINGS
                .iter()
                .map(|(dimension, member)| (Some(*dimension), Some(*member))),
        );

        for (dimension, member) in groupings {
            let case = format!("{mode} grouped by {dimension:?}");
            let mut body = request("Monthly", MARCH.0, MARCH.1);
            if let Some(dimension) = dimension {
                body["groupBy"] = json!(dimension);
            }
            let answered = groups(&ok(router, "ListEfficiencyMetrics", &body).await);

            let mut totalled: Vec<(Option<String>, i64, i64)> = answered
                .iter()
                .map(|group| {
                    let (spend, savings) = totals(group);
                    (label(group), spend, savings)
                })
                .collect();
            totalled.sort_by(|a, b| a.0.cmp(&b.0));
            let wanted = expected(&rows, member);
            assert_eq!(
                totalled, wanted,
                "case {case:?}: expected the groups {wanted:?}, got {totalled:?}"
            );

            // The score is the percentage of the group's spend its savings do
            // NOT remove, so a group with nothing left to save scores 100.
            for group in &answered {
                let (spend, savings) = totals(group);
                let expected_score =
                    100.0 - (savings as f64) / (spend as f64) * 100.0;
                assert!(
                    (score(group) - expected_score).abs() < 0.05,
                    "case {case:?} group {:?}: expected the score {expected_score}, got {}",
                    label(group),
                    score(group)
                );
                assert!(
                    (0.0..=100.0).contains(&score(group)),
                    "case {case:?} group {:?}: a score is a percentage, got {}",
                    label(group),
                    score(group)
                );
            }
        }
    }

    // The mode changes the figures, so the two runs above were not the same
    // answer twice.
    let body = request("Monthly", MARCH.0, MARCH.1);
    let before = groups(&ok(&default_mode, "ListEfficiencyMetrics", &body).await);
    let after = groups(&ok(&discounted, "ListEfficiencyMetrics", &body).await);
    assert_ne!(
        totals(&before[0]),
        totals(&after[0]),
        "case the two modes: expected the discounted mode to report different figures, \
         got {before:?} and {after:?}"
    );
}

/// The savings the no-grouping answer reports are the de-duped savings
/// `ListRecommendationSummaries` totals over the same rows, so the two
/// aggregated views of the catalog cannot disagree.
#[tokio::test]
async fn the_ungrouped_savings_are_the_summaries_deduped_total() {
    let router = enrolled_router().await;

    let summarised = ok(
        &router,
        "ListRecommendationSummaries",
        &json!({"groupBy": "ResourceType"}),
    )
    .await;
    let deduped_total = cents(
        summarised["estimatedTotalDedupedSavings"]
            .as_f64()
            .unwrap_or_default(),
    );

    let answered = groups(
        &ok(
            &router,
            "ListEfficiencyMetrics",
            &request("Monthly", MARCH.0, MARCH.1),
        )
        .await,
    );
    let (_, savings) = totals(&answered[0]);
    assert_eq!(
        savings, deduped_total,
        "case one month: expected the summaries' de-duped total {deduped_total}, got {savings}"
    );
}

/// A group whose resources have no spend over the period has nothing to compute
/// a score or a share from, so it reports no series and the message saying why —
/// which is how an account that has not enrolled reads, whatever granularity it
/// asks for. Once its analysis is due the same request reports the series.
#[tokio::test]
async fn a_group_with_no_cost_and_usage_data_reports_no_series_and_says_why() {
    let not_enrolled = router();

    for granularity in ["Daily", "Monthly"] {
        let case = format!("not enrolled, {granularity}");
        let answered = groups(
            &ok(
                &not_enrolled,
                "ListEfficiencyMetrics",
                &request(granularity, MARCH.0, MARCH.1),
            )
            .await,
        );
        assert_eq!(
            answered.len(),
            1,
            "case {case:?}: a request with no grouping reports one group, got {answered:?}"
        );
        assert!(
            answered[0]["metricsByTime"].is_null(),
            "case {case:?}: expected no series where no metrics could be calculated, got {:?}",
            answered[0]
        );
        let message = answered[0]["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a message explaining why no metrics were calculated, \
             got {:?}",
            answered[0]
        );
        assert!(
            answered[0]["group"].is_null(),
            "case {case:?}: a request with no grouping reports no dimension value, got {:?}",
            answered[0]
        );
    }

    // A grouped read of the same account has no dimension value to report at
    // all, so it carries no group.
    let grouped = groups(
        &ok(
            &not_enrolled,
            "ListEfficiencyMetrics",
            &{
                let mut body = request("Monthly", MARCH.0, MARCH.1);
                body["groupBy"] = json!("Region");
                body
            },
        )
        .await,
    );
    assert!(
        grouped.is_empty(),
        "case not enrolled, grouped: expected no groups, got {grouped:?}"
    );

    // The complement: an enrolled account whose analysis is due reports a series
    // and no message.
    let enrolled = enrolled_router().await;
    let answered = groups(
        &ok(
            &enrolled,
            "ListEfficiencyMetrics",
            &request("Monthly", MARCH.0, MARCH.1),
        )
        .await,
    );
    assert_eq!(
        stamps(&answered[0]),
        vec!["2026-03".to_string()],
        "case enrolled: expected the month's point, got {answered:?}"
    );
    assert!(
        answered[0]["message"].as_str().unwrap_or_default().is_empty(),
        "case enrolled: expected no message beside a series, got {:?}",
        answered[0]
    );
}

// ---------------------------------------------------------------------------
// Ordering and paging the groups
// ---------------------------------------------------------------------------

/// The groups come out in the order the request asked for: by the dimension it
/// named, in the direction it named — and by their dimension value where it
/// named no ordering at all, which is a total order and so a stable place for a
/// page to resume at.
#[tokio::test]
async fn the_groups_come_out_in_the_order_the_request_asked_for() {
    let router = enrolled_router().await;

    /// The figure one metric dimension orders on, read out of the answer itself,
    /// in whole cents.
    fn metric(group: &Value, dimension: &str) -> i64 {
        let (spend, savings) = totals(group);
        match dimension {
            "spend" => spend,
            "savings" => savings,
            "score" => cents(score(group)),
            other => panic!("case metric {other:?}: no figure to order by"),
        }
    }

    for dimension in ["group", "score", "savings", "spend", "SPEND"] {
        for (order, descending) in [("Asc", false), ("Desc", true)] {
            let case = format!("{dimension} {order}");
            let mut body = request("Monthly", MARCH.0, MARCH.1);
            body["groupBy"] = json!("ResourceType");
            body["orderBy"] = json!({"dimension": dimension, "order": order});
            let answered = groups(&ok(&router, "ListEfficiencyMetrics", &body).await);
            assert!(
                answered.len() > 1,
                "case {case:?}: expected more than one group to order, got {answered:?}"
            );

            for pair in answered.windows(2) {
                let (first, next) = (&pair[0], &pair[1]);
                // The dimension value is the group's own identity, so ordering
                // by it is ordering the labels; a metric dimension orders by the
                // figure, with ties broken by the label so the order is total
                // whichever direction was asked for.
                let ordered = if dimension.eq_ignore_ascii_case("group") {
                    if descending {
                        label(first) >= label(next)
                    } else {
                        label(first) <= label(next)
                    }
                } else {
                    let (a, b) = (metric(first, &dimension.to_lowercase()), metric(next, &dimension.to_lowercase()));
                    if descending {
                        a > b || (a == b && label(first) <= label(next))
                    } else {
                        a < b || (a == b && label(first) <= label(next))
                    }
                };
                assert!(
                    ordered,
                    "case {case:?}: expected {:?} before {:?}, got the pair out of order",
                    label(first),
                    label(next)
                );
            }
        }
    }

    // No ordering asked for: by the dimension value, which every page resumes
    // at the same place in.
    let mut body = request("Monthly", MARCH.0, MARCH.1);
    body["groupBy"] = json!("ResourceType");
    let answered = groups(&ok(&router, "ListEfficiencyMetrics", &body).await);
    let labels: Vec<Option<String>> = answered.iter().map(label).collect();
    let mut sorted = labels.clone();
    sorted.sort();
    assert_eq!(
        labels, sorted,
        "case no orderBy: expected the groups by dimension value, got {labels:?}"
    );
}

/// `maxResults` and `nextToken` page over the GROUPS — which is what the model
/// paginates (`smithy_query(query="operation",
/// operation="ListEfficiencyMetrics")`: `items` = `efficiencyMetricsByGroup`) —
/// so walking the pages yields exactly the groups of the unpaged read, in the
/// same order, and every page carries each group's whole series.
#[tokio::test]
async fn the_groups_page_and_every_page_carries_the_whole_series() {
    let router = enrolled_router().await;
    let mut body = request("Monthly", "2026-01-01", "2026-04-01");
    body["groupBy"] = json!("ResourceType");

    let whole = groups(&ok(&router, "ListEfficiencyMetrics", &body).await);
    assert!(
        whole.len() > 1,
        "case unpaged: expected more than one group to page over, got {whole:?}"
    );

    for size in 1..=whole.len() {
        let mut walked: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        // Bounded by the groups the unpaged read answered, so a token that never
        // stops fails rather than hangs.
        for _ in 0..=whole.len() {
            let mut paged = body.clone();
            paged["maxResults"] = json!(size);
            if let Some(cursor) = &cursor {
                paged["nextToken"] = json!(cursor);
            }
            let payload = ok(&router, "ListEfficiencyMetrics", &paged).await;
            let page = groups(&payload);
            assert!(
                page.len() <= size,
                "case page size {size}: expected at most {size} groups, got {page:?}"
            );
            for group in &page {
                assert_eq!(
                    stamps(group).len(),
                    3,
                    "case page size {size}: expected the whole three-month series in every \
                     page, got {group:?}"
                );
            }
            walked.extend(page);
            match payload["nextToken"].as_str() {
                None => break,
                Some(next) => cursor = Some(next.to_string()),
            }
        }
        assert_eq!(
            walked, whole,
            "case page size {size}: expected paging to yield the unpaged groups {whole:?}, \
             got {walked:?}"
        );
    }
}

/// The other half of the pagination rule, for the groups this read pages over:
/// a `nextToken` this read did not issue names no group to resume at, so it is
/// refused with HTTP 400 `ValidationException` attributed to `nextToken` and
/// naming the operation that would have issued an acceptable token — rather than
/// silently served the first page again.
///
/// The table spans the forms such a value takes: not the encoding this service
/// carries tokens in, a well-formed encoding of a body it never writes, a token
/// it issues for a list of another kind, a token another paginated read of the
/// same service issued, the empty value, this read's own token with a byte
/// appended, and a token naming a group this list never handed one out for (the
/// first group, and one past the last).
#[tokio::test]
async fn a_token_this_read_did_not_issue_is_refused() {
    let router = enrolled_router().await;
    let mut body = request("Monthly", MARCH.0, MARCH.1);
    body["groupBy"] = json!("ResourceType");

    // A token this read genuinely issued, so a case can differ from it only in
    // the group it names.
    let mut first = body.clone();
    first["maxResults"] = json!(1);
    let paged = ok(&router, "ListEfficiencyMetrics", &first).await;
    let own = paged["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case setup: expected a token after a page of one group, got {paged}"))
        .to_string();
    let groups_total = groups(&ok(&router, "ListEfficiencyMetrics", &body).await).len();

    // A token another paginated read of this service issued.
    let other = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "maxResults": 1}),
    )
    .await;
    let from_other_list = other["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case setup: expected a token from ListRecommendations, got {other}"))
        .to_string();

    /// A token in the encoding `issued` is carried in, naming `group` instead —
    /// built from a token the read really handed out, so a case states only the
    /// group it is about.
    fn token_at(issued: &str, group: usize) -> String {
        let decoded = hex::decode(issued).expect("an issued token is hex");
        let text = String::from_utf8(decoded).expect("an issued token decodes to text");
        let (head, _) = text
            .rsplit_once(':')
            .expect("an issued token carries the group it resumes at last");
        hex::encode(format!("{head}:{group}"))
    }

    let cases: Vec<(String, String)> = vec![
        ("not the token encoding".to_string(), "not-a-token".to_string()),
        (
            "a well-formed encoding of a body this service never issues".to_string(),
            hex::encode("page=1"),
        ),
        (
            "a token for a list of another kind".to_string(),
            cost_optimization_hub::state::encode_page_token("no-such-list", 1),
        ),
        (
            "a token issued by another paginated read".to_string(),
            from_other_list,
        ),
        ("the empty value".to_string(), String::new()),
        (
            "this read's own token with a byte appended".to_string(),
            format!("{own}00"),
        ),
        (
            "a token naming the first group".to_string(),
            token_at(&own, 0),
        ),
        (
            "a token naming the group one past the last".to_string(),
            token_at(&own, groups_total),
        ),
    ];

    for (case, token) in &cases {
        let mut refused = body.clone();
        refused["nextToken"] = json!(token);
        let (status, payload) = call(&router, "ListEfficiencyMetrics", &refused).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_stated_rule_refusal(&payload, case, "nextToken");
        let message = payload["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("ListEfficiencyMetrics"),
            "case {case:?}: expected the refusal to name the list that issues an acceptable \
             token, got {message:?}"
        );
        assert!(
            payload["efficiencyMetricsByGroup"].is_null(),
            "case {case:?}: expected a refusal rather than a page of groups, got {payload}"
        );
    }

    // The complement, from the same table: the token this read issued resumes at
    // the group the page that issued it stopped at.
    let mut resumed = body.clone();
    resumed["nextToken"] = json!(own);
    let rest = groups(&ok(&router, "ListEfficiencyMetrics", &resumed).await);
    let whole = groups(&ok(&router, "ListEfficiencyMetrics", &body).await);
    assert_eq!(
        rest,
        whole[1..].to_vec(),
        "case an issued token: expected the groups from the second on, got {rest:?}"
    );
}

// ---------------------------------------------------------------------------
// The requests this operation cannot report
// ---------------------------------------------------------------------------

/// A request this operation cannot report is refused with HTTP 400
/// `ValidationException`, whose `message` states the rule and whose `fields`
/// names the member the rule stopped — rather than served metrics about another
/// period, another grouping or another order.
#[tokio::test]
async fn a_request_this_operation_cannot_report_is_refused() {
    let router = enrolled_router().await;

    // (case, the request, the member the refusal names)
    let mut cases: Vec<(String, Value, &str)> = vec![
        (
            "a groupBy naming no dimension of a recommendation".to_string(),
            json!({"groupBy": "NotADimension"}),
            "groupBy",
        ),
        (
            // A per-resource identifier is no grouping: one group per resource
            // is not an aggregate.
            "a groupBy naming a per-resource identifier".to_string(),
            json!({"groupBy": "ResourceId"}),
            "groupBy",
        ),
        (
            "a blank groupBy".to_string(),
            json!({"groupBy": "   "}),
            "groupBy",
        ),
        (
            "an orderBy naming no dimension".to_string(),
            json!({"orderBy": {"order": "Asc"}}),
            "orderBy.dimension",
        ),
        (
            "an orderBy naming a member these groups do not carry".to_string(),
            json!({"orderBy": {"dimension": "accountId", "order": "Asc"}}),
            "orderBy.dimension",
        ),
        (
            "a period whose start is its end".to_string(),
            json!({"timePeriod": {"start": "2026-03-01", "end": "2026-03-01"}}),
            "timePeriod.end",
        ),
        (
            "a period that runs backwards".to_string(),
            json!({"timePeriod": {"start": "2026-04-01", "end": "2026-03-01"}}),
            "timePeriod.end",
        ),
        (
            "a period whose months collapse to the same month".to_string(),
            json!({"timePeriod": {"start": "2026-03", "end": "2026-03"}}),
            "timePeriod.end",
        ),
        (
            // A daily series longer than the operation reports for one group:
            // the response pages over groups, so there is no member to resume a
            // truncated series at.
            "a daily period longer than the series this operation reports".to_string(),
            json!({"timePeriod": {"start": "2020-01-01", "end": "2026-01-01"}}),
            "timePeriod",
        ),
    ];
    // Every form of a date this operation cannot read, on either end of the
    // period.
    for (member, other) in [("start", "end"), ("end", "start")] {
        for value in [
            "not-a-date",
            "2026-13-01",
            "2026-02-30",
            "2026",
            "",
            "03/01/2026",
            "2026-03-01T00:00:00Z",
        ] {
            let mut period = json!({"start": "2026-03-01", "end": "2026-04-01"});
            period[member] = json!(value);
            period[other] = json!(if member == "start" {
                "2026-04-01"
            } else {
                "2026-03-01"
            });
            cases.push((
                format!("a {member} this operation cannot read: {value:?}"),
                json!({"timePeriod": period}),
                if member == "start" {
                    "timePeriod.start"
                } else {
                    "timePeriod.end"
                },
            ));
        }
    }

    for (case, overrides, member) in &cases {
        // Each case is a well-formed request with the one member it is about
        // replaced, so nothing else can be what the refusal is answering.
        let mut body = request("Daily", "2026-03-01", "2026-03-04");
        for (key, value) in overrides.as_object().expect("an object of overrides") {
            body[key] = value.clone();
        }
        let (status, payload) = call(&router, "ListEfficiencyMetrics", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        assert_stated_rule_refusal(&payload, case, member);
        assert!(
            payload["efficiencyMetricsByGroup"].is_null(),
            "case {case:?}: expected a refusal rather than metrics, got {payload}"
        );
    }
}

/// Reading the metrics does not change what a later caller sees: two identical
/// reads, either side of every other read of the catalog, answer the same
/// thing — and the catalog reads the same afterwards.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let router = enrolled_router().await;
    let mut body = request("Daily", "2026-03-01", "2026-03-05");
    body["groupBy"] = json!("Region");

    let catalog_before = deduped(&router).await;
    let first = ok(&router, "ListEfficiencyMetrics", &body).await;
    ok(&router, "ListEfficiencyMetrics", &body).await;
    let second = ok(&router, "ListEfficiencyMetrics", &body).await;
    let catalog_after = deduped(&router).await;

    assert_eq!(
        first, second,
        "case repeated read: the metrics changed between two reads"
    );
    assert_eq!(
        catalog_before, catalog_after,
        "case repeated read: reading the metrics changed the catalog"
    );
}
