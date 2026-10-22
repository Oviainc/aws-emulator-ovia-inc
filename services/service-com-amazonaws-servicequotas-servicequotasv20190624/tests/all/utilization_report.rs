// SPDX-License-Identifier: CC0-1.0

//! The Service Quotas quota-utilization-report workflow, end to end over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine — so what these suites exercise is
//! the HTTP behaviour and the persisted state behind it, not a handler called
//! directly.
//!
//! The suites assert RULES over a table of inputs, not one recorded case:
//!
//! * a started report is `PENDING`, publishes nothing until the clock has made its
//!   generation due, and then reports `COMPLETED` with the same records however often
//!   it is read;
//! * every record in a report agrees, field for field, with what the quota reads
//!   report for the same `(ServiceCode, QuotaCode)` pair — for EVERY pair in the
//!   catalog — and carries a `Utilization` exactly when that quota publishes a usage
//!   metric;
//! * the report is ordered by utilization descending and is walked to the same
//!   records whatever page size is used, with page boundaries that do not move;
//! * a report is a SNAPSHOT: an increase approved after it was started does not change
//!   what it publishes, while the next report picks the new value up;
//! * a `ReportId` nothing minted, one from another region, and a token from another
//!   report are each refused with the error the model declares.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

// The wire harness this crate already has: `POST /` with `X-Amz-Target:
// ServiceQuotasV20190624.<Op>` through the `MultiServiceRouter`, over one shared
// engine, plus its listing walker and token helper. Declared `pub(crate)` in
// `quota_workflow` for sibling suites to reuse.
use crate::quota_workflow::{base64_of, client, emulator, walk_pages, Client};

/// Comfortably past `crate::state::REPORT_COMPLETION_DELAY` (500 ms), so a report
/// started before this wait has completed afterwards.
const PAST_COMPLETION: Duration = Duration::from_millis(1_200);

/// Comfortably past `crate::state::REQUEST_DECISION_DELAY` (500 ms).
const PAST_DECISION: Duration = Duration::from_millis(1_200);

/// Start a report and return its `ReportId`.
async fn start_report(client: &Client) -> String {
    let started = client.ok("StartQuotaUtilizationReport", json!({})).await;
    started["ReportId"]
        .as_str()
        .unwrap_or_else(|| panic!("a started report must report a ReportId, got {started}"))
        .to_string()
}

/// Walk a report to its end and return every record it yielded.
///
/// Asserts the page-size contract on the way: no page exceeds `MaxResults`, every page
/// but the last carries a `NextToken`, and the whole walk reports one `TotalCount`.
async fn walk_report(client: &Client, report_id: &str, page_size: Option<i64>) -> Vec<Value> {
    let case = format!("report {report_id} at page size {page_size:?}");
    let mut records = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut body = json!({"ReportId": report_id});
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let page = client.ok("GetQuotaUtilizationReport", body).await;
        pages += 1;
        // Bounded so a report that never reports its last page fails instead of
        // hanging: the whole report walked ONE record at a time, with headroom.
        assert!(pages <= 2_048, "case {case}: the walk must terminate");

        assert_eq!(
            page["Status"],
            json!("COMPLETED"),
            "case {case}: a completed report must stay completed, got {page}"
        );
        let total = page["TotalCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: a completed report reports a TotalCount: {page}"));
        let quotas = page["Quotas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: a completed report reports Quotas: {page}"))
            .clone();
        if let Some(size) = page_size {
            assert!(
                quotas.len() as i64 <= size,
                "case {case}: a page must not exceed MaxResults, got {} records",
                quotas.len()
            );
        }
        records.extend(quotas);
        token = page["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            assert_eq!(
                records.len() as i64,
                total,
                "case {case}: the walk must yield TotalCount records, got {}",
                records.len()
            );
            break;
        }
    }
    records
}

/// The `(ServiceCode, QuotaCode)` key of one record, for comparing two orderings.
fn key_of(record: &Value) -> (String, String) {
    (
        record["ServiceCode"].as_str().unwrap_or_default().to_string(),
        record["QuotaCode"].as_str().unwrap_or_default().to_string(),
    )
}

/// Every quota in the catalog, as the implemented quota reads report it: the applied
/// view and the AWS default view, keyed by `(ServiceCode, QuotaCode)`.
async fn catalog(client: &Client) -> BTreeMap<(String, String), (Value, Value)> {
    // Both listings are walked to their end: the catalog is several hundred quotas
    // across several dozen services, so a single page is not all of it.
    let services = walk_pages(client, "ListServices", &json!({}), "Services", None).await;
    let mut quotas = BTreeMap::new();
    for service in &services {
        let service_code = service["ServiceCode"]
            .as_str()
            .expect("a listed service has a code")
            .to_string();
        let listed = walk_pages(
            client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await;
        for quota in &listed {
            let quota_code = quota["QuotaCode"]
                .as_str()
                .expect("a listed quota has a code")
                .to_string();
            let default = client
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            quotas.insert(
                (service_code.clone(), quota_code),
                (quota.clone(), default["Quota"].clone()),
            );
        }
    }
    assert!(
        !quotas.is_empty(),
        "the seeded catalog must hold at least one quota"
    );
    quotas
}

#[tokio::test]
async fn a_report_is_pending_when_started_and_completes_on_its_own_clock() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: the report is started twice, so the rule is asserted on a report that is
    // the account's first and on one started while another already exists.
    for case in ["the first report", "a second report"] {
        let started = client.ok("StartQuotaUtilizationReport", json!({})).await;
        let report_id = started["ReportId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ReportId, got {started}"))
            .to_string();

        assert_eq!(
            started["Status"],
            json!("PENDING"),
            "case {case}: a just-initiated report must be PENDING, got {started}"
        );
        // The modelled `ReportId` pattern `^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$` and its
        // 1..128 length.
        assert!(
            (2..=128).contains(&report_id.len())
                && report_id.chars().next().is_some_and(char::is_alphanumeric)
                && report_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case}: the ReportId must match the modelled pattern, got {report_id:?}"
        );

        // A caller polling straight after the start still sees the transient state,
        // and an unfinished report publishes no records.
        let polled = client
            .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
            .await;
        assert!(
            polled["Status"] == json!("PENDING") || polled["Status"] == json!("IN_PROGRESS"),
            "case {case}: a report just started must not be COMPLETED, got {polled}"
        );
        assert!(
            polled["Quotas"].is_null() && polled["TotalCount"].is_null(),
            "case {case}: an unfinished report must publish no records, got {polled}"
        );
        assert_eq!(
            polled["ReportId"],
            json!(report_id),
            "case {case}: the read must name the report asked for, got {polled}"
        );

        tokio::time::sleep(PAST_COMPLETION).await;

        let completed = client
            .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
            .await;
        assert_eq!(
            completed["Status"],
            json!("COMPLETED"),
            "case {case}: the report must complete on its own clock, got {completed}"
        );
        assert!(
            completed["GeneratedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: a completed report must report GeneratedAt, got {completed}"
        );
        assert!(
            completed["ErrorCode"].is_null() && completed["ErrorMessage"].is_null(),
            "case {case}: a report that did not fail carries no error, got {completed}"
        );

        // Reading twice answers the same thing: a read publishes nothing new.
        let again = client
            .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
            .await;
        assert_eq!(
            again, completed,
            "case {case}: a second read must answer the same, got {again}"
        );
    }
}

#[tokio::test]
async fn two_reports_are_two_resources() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let first = start_report(&client).await;
    let second = start_report(&client).await;
    assert_ne!(
        first, second,
        "case two starts: each start must mint its own report, got {first} twice"
    );

    tokio::time::sleep(PAST_COMPLETION).await;

    // Rows: both reports, each read on its own id.
    for (case, report_id) in [("the first report", &first), ("the second report", &second)] {
        let read = client
            .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
            .await;
        assert_eq!(
            read["ReportId"],
            json!(report_id),
            "case {case}: a read must answer for the id it was given, got {read}"
        );
        assert_eq!(
            read["Status"],
            json!("COMPLETED"),
            "case {case}: expected a completed report, got {read}"
        );
    }
}

#[tokio::test]
async fn every_report_record_agrees_with_the_quota_reads_for_the_same_pair() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let expected = catalog(&client).await;
    let report_id = start_report(&client).await;
    tokio::time::sleep(PAST_COMPLETION).await;
    let records = walk_report(&client, &report_id, None).await;

    assert_eq!(
        records.len(),
        expected.len(),
        "case the whole catalog: the report must cover every quota, got {records:?}"
    );

    for record in &records {
        let key = key_of(record);
        let case = format!("{}/{}", key.0, key.1);
        let (applied, default) = expected
            .get(&key)
            .unwrap_or_else(|| panic!("case {case}: the report names a quota no read returns"));

        assert_eq!(
            record["QuotaName"], applied["QuotaName"],
            "case {case}: the record must carry the quota's name, got {record}"
        );
        assert_eq!(
            record["ServiceName"], applied["ServiceName"],
            "case {case}: the record must carry the service's name, got {record}"
        );
        assert_eq!(
            record["AppliedValue"].as_f64(),
            applied["Value"].as_f64(),
            "case {case}: AppliedValue must be the value in force, got {record}"
        );
        assert_eq!(
            record["DefaultValue"].as_f64(),
            default["Value"].as_f64(),
            "case {case}: DefaultValue must be the AWS default, got {record}"
        );
        assert_eq!(
            record["Adjustable"], applied["Adjustable"],
            "case {case}: Adjustable must agree with the quota read, got {record}"
        );

        // Service Quotas tracks usage through the CloudWatch usage metric a quota
        // publishes, so the record carries the namespace of THAT metric, and carries a
        // Utilization exactly when there is a metric to compute one from.
        let metric_namespace = applied["UsageMetric"]["MetricNamespace"].clone();
        assert_eq!(
            record["Namespace"], metric_namespace,
            "case {case}: Namespace must be the usage metric's namespace, got {record}"
        );
        assert_eq!(
            record["Utilization"].is_null(),
            metric_namespace.is_null(),
            "case {case}: a Utilization is reported exactly for a quota with a usage \
             metric, got {record}"
        );
        if let Some(utilization) = record["Utilization"].as_f64() {
            let applied_value = record["AppliedValue"].as_f64().unwrap_or_default();
            // Utilization is usage over the value in force; nothing in this emulator
            // consumes a quota, so the usage it has recorded against one is zero. A
            // quota whose value in force is zero (AWS publishes several) is no
            // denominator: with nothing used, none of what it permits is used.
            let recorded_usage = 0.0_f64;
            let expected = if applied_value > 0.0 {
                recorded_usage / applied_value * 100.0
            } else {
                0.0
            };
            assert!(
                (0.0..=100.0).contains(&utilization),
                "case {case}: utilization must be a percentage, got {utilization}"
            );
            assert_eq!(
                utilization, expected,
                "case {case}: utilization must be the recorded usage over the applied \
                 value, got {record}"
            );
        }
    }
}

#[tokio::test]
async fn a_report_is_ordered_by_utilization_descending_and_pages_consistently() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    let report_id = start_report(&client).await;
    tokio::time::sleep(PAST_COMPLETION).await;

    let whole = walk_report(&client, &report_id, None).await;

    // Highest utilization first, and a record with no utilization to report last.
    let mut seen_missing = false;
    let mut previous: Option<f64> = None;
    for record in &whole {
        let case = format!("{}/{}", key_of(record).0, key_of(record).1);
        match record["Utilization"].as_f64() {
            Some(utilization) => {
                assert!(
                    !seen_missing,
                    "case {case}: a record with a utilization must not follow one without, \
                     got {record}"
                );
                if let Some(previous) = previous {
                    assert!(
                        utilization <= previous,
                        "case {case}: expected utilization at most {previous}, got {utilization}"
                    );
                }
                previous = Some(utilization);
            }
            None => seen_missing = true,
        }
    }

    // Rows: page sizes spanning one record per page, several, exactly the record
    // count, the modelled maximum (1000) and no MaxResults at all.
    let sizes: Vec<Option<i64>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(whole.len() as i64),
        Some(1_000),
        None,
    ];
    for size in sizes {
        let walked = walk_report(&client, &report_id, size).await;
        assert_eq!(
            walked, whole,
            "case page size {size:?}: a report must walk to the same records in the same \
             order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_report_is_a_snapshot_of_the_quotas_when_it_was_started() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let pair = json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE"});
    let key = ("vpc".to_string(), "L-F678F1CE".to_string());

    let before_value = client.ok("GetServiceQuota", pair.clone()).await["Quota"]["Value"]
        .as_f64()
        .expect("the quota reports a Value");

    let before = start_report(&client).await;

    // An increase approved AFTER the report was started.
    let desired = before_value + 15.0;
    client
        .ok(
            "RequestServiceQuotaIncrease",
            json!({"ServiceCode": "vpc", "QuotaCode": "L-F678F1CE", "DesiredValue": desired}),
        )
        .await;
    tokio::time::sleep(PAST_DECISION).await;
    assert_eq!(
        client.ok("GetServiceQuota", pair.clone()).await["Quota"]["Value"].as_f64(),
        Some(desired),
        "case the increase: the approved value must be in force before the next report"
    );

    let after = start_report(&client).await;
    tokio::time::sleep(PAST_COMPLETION).await;

    // Rows: the report started before the increase, and the one started after it.
    for (case, report_id, expected) in [
        ("started before the increase", &before, before_value),
        ("started after the increase", &after, desired),
    ] {
        let records = walk_report(&client, report_id, Some(4)).await;
        let record = records
            .iter()
            .find(|record| key_of(record) == key)
            .unwrap_or_else(|| panic!("case {case}: the report must cover {key:?}"));
        assert_eq!(
            record["AppliedValue"].as_f64(),
            Some(expected),
            "case {case}: expected the value in force when the report was started, got {record}"
        );
        assert_eq!(
            record["DefaultValue"].as_f64(),
            Some(before_value),
            "case {case}: an increase must never change the AWS default, got {record}"
        );
    }
}

#[tokio::test]
async fn a_report_this_emulator_did_not_mint_is_refused() {
    let engine = emulator();
    let elsewhere = client(&engine, "eu-west-1");
    let home = client(&engine, "us-east-1");

    let mine = start_report(&home).await;
    let other = start_report(&home).await;
    tokio::time::sleep(PAST_COMPLETION).await;
    let token = home
        .ok(
            "GetQuotaUtilizationReport",
            json!({"ReportId": other, "MaxResults": 1}),
        )
        .await["NextToken"]
        .as_str()
        .expect("a first page of a multi-page report carries a NextToken")
        .to_string();

    // Rows: a report id nothing minted, one minted in another region, and tokens no
    // read of THIS report issued.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a ReportId nothing minted",
            json!({"ReportId": "deadbeefdeadbeefdeadbeefdeadbeef"}),
            "NoSuchResourceException",
        ),
        (
            "the shortest ReportId the model accepts",
            json!({"ReportId": "ab"}),
            "NoSuchResourceException",
        ),
        (
            "a token another report issued",
            json!({"ReportId": mine, "NextToken": token}),
            "InvalidPaginationTokenException",
        ),
        (
            "a token that is not base64",
            json!({"ReportId": mine, "NextToken": "not+a+token"}),
            "InvalidPaginationTokenException",
        ),
        (
            "a token another listing issued",
            json!({"ReportId": mine, "NextToken": base64_of("ListServices:1")}),
            "InvalidPaginationTokenException",
        ),
        (
            "a token naming a position past the report",
            json!({
                "ReportId": mine,
                "NextToken": base64_of(&format!("GetQuotaUtilizationReport:{mine}:9999")),
            }),
            "InvalidPaginationTokenException",
        ),
    ];
    for (case, body, expected) in rows {
        let (status, code) = home.refused("GetQuotaUtilizationReport", body).await;
        assert_eq!(
            (status, code.contains(expected)),
            (400, true),
            "case {case}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }

    // A report belongs to the region it was started in: another region's caller does
    // not see it, and its own caller still does.
    let (status, code) = elsewhere
        .refused("GetQuotaUtilizationReport", json!({"ReportId": mine}))
        .await;
    assert_eq!(
        (status, code.contains("NoSuchResourceException")),
        (400, true),
        "case another region's caller: expected HTTP 400 NoSuchResourceException, got \
         {status} {code}"
    );
    let still_there = home
        .ok("GetQuotaUtilizationReport", json!({"ReportId": mine}))
        .await;
    assert_eq!(
        still_there["Status"],
        json!("COMPLETED"),
        "case the owning region: a refused read elsewhere must not disturb the report, got \
         {still_there}"
    );
}

/// The report store has a ceiling, and a start past it is refused with the
/// `TooManyRequestsException` the model declares — per region.
#[tokio::test]
async fn the_report_store_is_bounded_and_refuses_past_its_ceiling() {
    use service_com_amazonaws_servicequotas_servicequotasv20190624::state;

    let engine = emulator();
    let elsewhere = client(&engine, "eu-west-1");
    let home = client(&engine, "us-east-1");

    // Fill one region to its ceiling over the wire.
    let first = start_report(&home).await;
    for index in 1..state::MAX_LIVE_REPORTS_PER_REGION {
        let started = home.ok("StartQuotaUtilizationReport", json!({})).await;
        assert_eq!(
            started["Status"],
            json!("PENDING"),
            "case start {index}: a start below the ceiling must be accepted, got {started}"
        );
    }

    let (status, code) = home.refused("StartQuotaUtilizationReport", json!({})).await;
    assert_eq!(
        (status, code.contains("TooManyRequestsException")),
        (400, true),
        "case a start past the ceiling: expected HTTP 400 TooManyRequestsException, got \
         {status} {code}"
    );

    // A refused start leaves the reports already in the store readable.
    let read = home
        .ok("GetQuotaUtilizationReport", json!({"ReportId": first}))
        .await;
    assert_eq!(
        read["ReportId"],
        json!(first),
        "case the first report: a refused start must not disturb it, got {read}"
    );

    // The ceiling is per region, like the rest of this service's mutable state.
    let accepted = elsewhere.ok("StartQuotaUtilizationReport", json!({})).await;
    assert_eq!(
        accepted["Status"],
        json!("PENDING"),
        "case another region: its own allowance must be untouched, got {accepted}"
    );
}

