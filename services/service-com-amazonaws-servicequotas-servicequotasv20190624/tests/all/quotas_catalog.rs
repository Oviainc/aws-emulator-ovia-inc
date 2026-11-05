// SPDX-License-Identifier: CC0-1.0

//! The quota CATALOG contract: which record a read answers with, what that record
//! carries, and the order a listing reports.
//!
//! These suites assert RULES over the WHOLE seeded catalog — every
//! `(ServiceCode, QuotaCode)` pair of every service that publishes quotas is a row,
//! which spans the adjustable and the fixed quotas, the counted and the measured
//! ones, the rate-shaped and the plain, the regional and the global — rather than one
//! recorded quota:
//!
//! * a read answers with the row for EXACTLY the pair it was asked for, and two
//!   distinct codes of one service never answer with the same record;
//! * whatever a row publishes — `Description`, `Period`, `UsageMetric` — is reported,
//!   within the modelled constraints, and identically by the single read, the
//!   listing, and the applied and default views;
//! * a listing reports the catalog's PUBLISHED order (not the `QuotaCode` order),
//!   whatever page size it is walked with, and carries a `NextToken` for exactly as
//!   long as rows remain after the page it served — the size a CALLER asked for with
//!   `MaxResults`, else that listing's own unasked page.
//!
//! The ARN rule of the two views (an AWS default's account segment is empty) is
//! pinned by `default_quota_catalog.rs`; the harness (`Client`, `walk_pages`) is the
//! one `quota_workflow.rs` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::APPLIED_QUOTA_PAGE_SIZE;

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// The `PeriodUnit` members, from the model
/// (`shared_types::PERIOD_UNIT_VALUES`, i.e. `smithy_query(query="shape",
/// shape="PeriodUnit")`).
const PERIOD_UNITS: &[&str] = &[
    "DAY",
    "HOUR",
    "MICROSECOND",
    "MILLISECOND",
    "MINUTE",
    "SECOND",
    "WEEK",
];

/// The `Statistic` values the model's pattern `^(Sum|Maximum)$` allows.
const STATISTICS: &[&str] = &["Sum", "Maximum"];

/// `QuotaDescription` is modelled `@length 0..350`, pattern `^.{0,350}$`.
const MAX_DESCRIPTION: usize = 350;

/// `MetricDimensionsMapDefinition` is modelled `@length 0..10`.
const MAX_METRIC_DIMENSIONS: usize = 10;

/// The members a quota publishes only when its own row defines them.
const OPTIONAL_MEMBERS: &[&str] = &["Description", "Period", "UsageMetric"];

/// How long a service's listing may be before the one-row-per-page walk is skipped
/// for it: a catalog service such as EC2 publishes hundreds of quotas, and walking
/// those one row at a time says nothing the shorter services have not already said.
const ONE_ROW_WALK_LIMIT: usize = 40;

/// Every service that publishes quotas, with its quotas in listing order.
///
/// A catalog service whose published defaults this emulator does not carry answers
/// 200 with an EMPTY `Quotas` list (`state::seed_default_quotas`), so it has no row
/// here; `default_quota_catalog.rs` is what pins that empty-list rule.
async fn catalog(client: &Client) -> Vec<(String, Vec<Value>)> {
    let services = walk_pages(client, "ListServices", &json!({}), "Services", None).await;
    let mut rows = Vec::new();
    for service in &services {
        let service_code = service["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
            .to_string();
        let quotas = walk_pages(
            client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        if quotas.is_empty() {
            continue;
        }
        rows.push((service_code, quotas));
    }
    assert!(
        rows.len() >= 2,
        "case catalog: expected several services to publish quotas, got {}",
        rows.len()
    );
    rows
}

/// The `QuotaCode`s of a listing, in order.
fn codes(quotas: &[Value]) -> Vec<String> {
    quotas
        .iter()
        .map(|quota| {
            quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case listing: a quota without a code: {quota}"))
                .to_string()
        })
        .collect()
}

/// A single read answers with the row for exactly the pair asked for: never a
/// per-service default, and never another code's record.
#[tokio::test]
async fn a_read_answers_with_the_row_for_exactly_the_requested_quota_code() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let mut pairs = 0_usize;
    for (service_code, quotas) in catalog(&client).await {
        // A pair identifies a quota: within one service no code repeats, so no two
        // rows can be reached by the same request.
        let listed = codes(&quotas);
        let distinct: BTreeSet<&String> = listed.iter().collect();
        assert_eq!(
            distinct.len(),
            listed.len(),
            "case {service_code}: a QuotaCode must identify one quota, got {listed:?}"
        );

        for quota in &quotas {
            let quota_code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
            let case = format!("{service_code}/{quota_code}");
            let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});

            for operation in ["GetServiceQuota", "GetAWSDefaultServiceQuota"] {
                let record = client.ok(operation, pair.clone()).await["Quota"].clone();
                assert_eq!(
                    record["QuotaCode"],
                    json!(quota_code),
                    "case {case}: {operation} must answer for the requested QuotaCode, got \
                     {record}"
                );
                assert_eq!(
                    record["ServiceCode"],
                    json!(service_code),
                    "case {case}: {operation} must answer for the requested ServiceCode, got \
                     {record}"
                );
                // Everything the row fixes is the same on both views and in the
                // listing; only Value and the applied level may differ, and nothing
                // has been applied here.
                for member in [
                    "QuotaName",
                    "Unit",
                    "Adjustable",
                    "GlobalQuota",
                    "Description",
                    "Period",
                    "UsageMetric",
                ] {
                    assert_eq!(
                        record[member], quota[member],
                        "case {case}: {operation} and the listing must agree on {member}, got \
                         {record} and {quota}"
                    );
                }
            }
            pairs += 1;
        }
    }

    assert!(
        pairs > 50,
        "case catalog: expected the catalog to cover many quotas, covered {pairs}"
    );
}

/// Whatever a row publishes is reported, within the modelled constraints — and a row
/// that publishes none of the optional members says so by omitting them, rather than
/// by reporting an empty one.
#[tokio::test]
async fn a_quota_record_carries_the_members_its_row_publishes() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let mut with_description = 0_usize;
    let mut with_period = 0_usize;
    let mut with_metric = 0_usize;

    for (service_code, quotas) in catalog(&client).await {
        for quota in &quotas {
            let quota_code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
            let case = format!("{service_code}/{quota_code}");

            for member in OPTIONAL_MEMBERS {
                let value = &quota[member];
                assert!(
                    value.is_null() || !value.is_array(),
                    "case {case}: {member} must be omitted or a value, got {quota}"
                );
            }

            if let Some(description) = quota["Description"].as_str() {
                with_description += 1;
                assert!(
                    !description.is_empty() && description.chars().count() <= MAX_DESCRIPTION,
                    "case {case}: a published Description is 1..={MAX_DESCRIPTION} characters, \
                     got {} in {quota}",
                    description.chars().count()
                );
                assert!(
                    !description.contains('\n'),
                    "case {case}: Description must satisfy ^.{{0,350}}$, got {description:?}"
                );
            }

            if let Some(period) = quota["Period"].as_object() {
                with_period += 1;
                let unit = period
                    .get("PeriodUnit")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    PERIOD_UNITS.contains(&unit),
                    "case {case}: PeriodUnit must be a modelled member, got {unit:?}"
                );
                let value = period
                    .get("PeriodValue")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                assert!(
                    value >= 1,
                    "case {case}: PeriodValue must be a positive count of {unit}, got {value}"
                );
            }

            if let Some(metric) = quota["UsageMetric"].as_object() {
                with_metric += 1;
                assert!(
                    metric
                        .get("MetricNamespace")
                        .and_then(Value::as_str)
                        .is_some_and(|namespace| !namespace.is_empty()),
                    "case {case}: a UsageMetric names its namespace, got {quota}"
                );
                assert!(
                    metric
                        .get("MetricName")
                        .and_then(Value::as_str)
                        .is_some_and(|name| !name.is_empty()),
                    "case {case}: a UsageMetric names its metric, got {quota}"
                );
                let statistic = metric
                    .get("MetricStatisticRecommendation")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    STATISTICS.contains(&statistic),
                    "case {case}: MetricStatisticRecommendation must satisfy ^(Sum|Maximum)$, \
                     got {statistic:?}"
                );
                let dimensions = metric
                    .get("MetricDimensions")
                    .and_then(Value::as_object)
                    .map_or(0, serde_json::Map::len);
                assert!(
                    dimensions <= MAX_METRIC_DIMENSIONS,
                    "case {case}: MetricDimensions must hold at most \
                     {MAX_METRIC_DIMENSIONS} entries, got {dimensions} in {quota}"
                );
                // A dimension the metric does carry is a name/value pair that is part
                // of the metric's identity, so neither half may be empty. The map
                // itself is modelled `@length 0..10`: a service that publishes the
                // account's usage under ONE metric of its own namespace identifies it
                // without any dimension, and reports none.
                for (name, value) in metric
                    .get("MetricDimensions")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        !name.is_empty() && value.as_str().is_some_and(|value| !value.is_empty()),
                        "case {case}: a metric dimension is a name/value pair, got \
                         {name:?}={value}"
                    );
                }
            }
        }

        // The whole record survives a page boundary: walked one row at a time, the
        // listing yields the same records in the same order.
        if quotas.len() <= ONE_ROW_WALK_LIMIT {
            let one_at_a_time = walk_pages(
                &client,
                "ListServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                Some(1),
            )
            .await;
            assert_eq!(
                one_at_a_time, quotas,
                "case {service_code}: one-row pages must carry the same records"
            );
        }
    }

    assert!(
        with_description >= 1 && with_period >= 1 && with_metric >= 1,
        "case catalog: the catalog must publish rows carrying a Description, a Period and a \
         UsageMetric, got {with_description}, {with_period} and {with_metric}"
    );
}

/// A listing reports the catalog's PUBLISHED order — not the code-sorted order — and
/// carries a `NextToken` for exactly as long as rows remain.
#[tokio::test]
async fn a_listing_reports_the_published_order_and_pages_with_a_next_token() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let mut unsorted_services = 0_usize;

    for (service_code, quotas) in catalog(&client).await {
        let listed = codes(&quotas);

        // The applied and the default listing publish ONE order.
        let defaults = walk_pages(
            &client,
            "ListAWSDefaultServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        assert_eq!(
            codes(&defaults),
            listed,
            "case {service_code}: both listings must publish one order"
        );

        let mut sorted = listed.clone();
        sorted.sort();
        if sorted != listed {
            unsorted_services += 1;
        }

        // Rows: page sizes inside the modelled 1..100 range, including both ends,
        // skipping the one-row walk of a catalog service with hundreds of quotas.
        for size in [1_i64, 7, 99, 100] {
            if size == 1 && quotas.len() > ONE_ROW_WALK_LIMIT {
                continue;
            }
            let paged = walk_pages(
                &client,
                "ListServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                Some(size),
            )
            .await;
            assert_eq!(
                paged, quotas,
                "case {service_code} MaxResults={size}: paging must yield the published order"
            );

            // The first page stops at `size` rows and carries a token for exactly as
            // long as rows remain.
            let first = client
                .ok(
                    "ListServiceQuotas",
                    json!({"ServiceCode": service_code, "MaxResults": size}),
                )
                .await;
            let page = first["Quotas"].as_array().cloned().unwrap_or_default();
            let expected = (size as usize).min(quotas.len());
            assert_eq!(
                page.len(),
                expected,
                "case {service_code} MaxResults={size}: expected {expected} rows, got {first}"
            );
            assert_eq!(
                page,
                quotas[..expected],
                "case {service_code} MaxResults={size}: the first page must be the first rows"
            );
            assert_eq!(
                first["NextToken"].is_string(),
                expected < quotas.len(),
                "case {service_code} MaxResults={size}: a NextToken is present exactly while \
                 rows remain, got {first}"
            );
        }

        // A caller that names no MaxResults is served this listing's OWN unasked page
        // (`state::APPLIED_QUOTA_PAGE_SIZE`) — the first rows of the published order,
        // with a token after it exactly while rows remain: `MaxResults` bounds what a
        // caller may ask for, not what the listing serves when none is named.
        let defaulted = client
            .ok("ListServiceQuotas", json!({"ServiceCode": service_code}))
            .await;
        let unasked = defaulted["Quotas"].as_array().cloned().unwrap_or_default();
        let served = APPLIED_QUOTA_PAGE_SIZE.min(quotas.len());
        assert_eq!(
            unasked.len(),
            served,
            "case {service_code} no MaxResults: expected the first {served} of the service's \
             {} rows on the unasked page, got {defaulted}",
            quotas.len()
        );
        assert_eq!(
            unasked,
            quotas[..served],
            "case {service_code} no MaxResults: the unasked page must be the first rows of the \
             published order"
        );
        assert_eq!(
            defaulted["NextToken"].is_string(),
            served < quotas.len(),
            "case {service_code} no MaxResults: a NextToken follows the unasked page exactly \
             while rows remain, got {defaulted}"
        );
    }

    assert!(
        unsorted_services >= 1,
        "case catalog: at least one service must publish its quotas in an order that is not the \
         QuotaCode order — otherwise this test cannot tell a published order from a sorted key"
    );
}
