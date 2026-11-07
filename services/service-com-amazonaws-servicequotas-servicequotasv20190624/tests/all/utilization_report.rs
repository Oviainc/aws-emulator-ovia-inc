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
//! * a start while the region's report is still live hands THAT report back — its id
//!   and the status it carries now — and says so in `Message`, so a caller that
//!   re-starts converges on one report instead of accumulating them;
//! * every record in a report agrees, field for field, with what the quota reads
//!   report for the same `(ServiceCode, QuotaCode)` pair — for EVERY pair in the
//!   catalog — and carries a `Utilization` only where that quota publishes the usage
//!   metric its usage would be counted by (what the figure itself is, measured against
//!   an account that holds resources, is tabled in `quota_utilization_usage`);
//! * the report is ordered by utilization descending and is walked to the same
//!   records whatever page size is used, with page boundaries that do not move;
//! * a report is a SNAPSHOT: an increase approved after it was started does not change
//!   what it publishes, while a report started after it picks the new value up;
//! * a `ReportId` nothing minted, one from another region, and a token from another
//!   report are each refused with the error the model declares.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

// The wire harness this crate already has: `POST /` with `X-Amz-Target:
// ServiceQuotasV20190624.<Op>` through the `MultiServiceRouter`, over one shared
// engine, plus its listing walker and token helper. Declared `pub(crate)` in
// `quota_workflow` for sibling suites to reuse.
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::UTILIZATION_REPORT_PAGE_SIZE;

use crate::quota_workflow::{base64_of, client, emulator, walk_pages, Client};

/// Comfortably past `crate::state::REPORT_COMPLETION_DELAY` (500 ms), so a report
/// started before this wait has completed afterwards.
const PAST_COMPLETION: Duration = Duration::from_millis(1_200);

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open).
const PAST_DECISION: Duration = Duration::from_millis(1_800);

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

/// How many utilization reports the service has stored for `region`, read out of the
/// one shared engine these callers serve from.
fn stored_reports(engine: &std::sync::Arc<engine::InMemoryEngine>, region: &str) -> usize {
    use service_com_amazonaws_servicequotas_servicequotasv20190624::state;

    state::service_state(&**engine)
        .expect("the service keeps its state in the engine")
        .lock()
        .stored_report_count(region)
}

/// How far through its generation a `ReportStatus` is, so two observations of one
/// report can be compared without pinning the instant each was made at.
fn status_rank(status: &str) -> usize {
    match status {
        "PENDING" => 0,
        "IN_PROGRESS" => 1,
        "COMPLETED" => 2,
        other => panic!("a report must carry a modelled ReportStatus, got {other:?}"),
    }
}

/// A start's `Message` must say whether an existing recent report is being reused —
/// the one thing the modelled `ReportMessage` is documented to carry — and stay inside
/// the length the model declares for it (`ReportMessage`, len 0..350).
fn assert_report_message(started: &Value, reused: bool, case: &str) {
    let message = started["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a start must report a Message, got {started}"));
    assert!(
        message.chars().count() <= 350,
        "case {case}: the Message must stay inside the modelled 0..350 length, got {} \
         characters",
        message.chars().count()
    );
    assert_eq!(
        message.to_lowercase().contains("reus"),
        reused,
        "case {case}: expected a Message that {} an existing recent report is being \
         reused, got {message:?}",
        if reused { "says" } else { "does not say" }
    );
}

#[tokio::test]
async fn a_report_is_pending_when_started_and_completes_on_its_own_clock() {
    let engine = emulator();

    // Rows: two regional endpoints, each holding no report of its own yet, so each
    // start MINTS one. (A start made where a live report already exists reuses it
    // instead, which `a_start_reuses_the_live_report_and_mints_only_where_none_exists`
    // covers.)
    for region in ["us-east-1", "eu-west-1"] {
        let client = client(&engine, region);
        let case = format!("the first report in {region}");
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
        assert_report_message(&started, false, &case);
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

/// A start hands back the region's live report instead of minting a second one,
/// whatever status the clock has brought it to, and says so in `Message`; only a
/// region holding no live report mints one.
#[tokio::test]
async fn a_start_reuses_the_live_report_and_mints_only_where_none_exists() {
    let engine = emulator();
    let home = client(&engine, "us-east-1");
    let elsewhere = client(&engine, "ap-south-1");

    // Nothing to reuse yet, so this start mints a report.
    let minted = home.ok("StartQuotaUtilizationReport", json!({})).await;
    let report_id = minted["ReportId"]
        .as_str()
        .unwrap_or_else(|| panic!("case the first start: expected a ReportId, got {minted}"))
        .to_string();
    assert_eq!(
        minted["Status"],
        json!("PENDING"),
        "case the first start: a minted report must be PENDING, got {minted}"
    );
    assert_report_message(&minted, false, "the first start");

    // Rows: a re-start while the report is still generating and one after the clock
    // has completed it — the statuses a live report can carry.
    let rows: Vec<(&str, Option<Duration>, Vec<&str>)> = vec![
        (
            "a re-start while the report is still generating",
            None,
            vec!["PENDING", "IN_PROGRESS"],
        ),
        (
            "a re-start after the report has completed",
            Some(PAST_COMPLETION),
            vec!["COMPLETED"],
        ),
    ];
    for (case, wait, expected) in rows {
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
        let restarted = home.ok("StartQuotaUtilizationReport", json!({})).await;
        assert_eq!(
            restarted["ReportId"],
            json!(report_id),
            "case {case}: expected the live report {report_id}, got {restarted}"
        );
        let status = restarted["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            expected.contains(&status.as_str()),
            "case {case}: expected one of {expected:?}, got {status:?}"
        );
        // The start answers with the status a read of the same report answers with at
        // the same moment: it is ONE resource, reached two ways.
        let read = home
            .ok("GetQuotaUtilizationReport", json!({"ReportId": report_id}))
            .await;
        // The start and the read are two observations of ONE resource, so the read
        // (made after the start) is at the same point of its generation or further
        // along it — never behind, and never a different report.
        let read_status = read["Status"].as_str().unwrap_or_default();
        assert!(
            status_rank(read_status) >= status_rank(&status),
            "case {case}: a read after the start must not be behind it, got {read_status:?} \
             after {status:?}"
        );
        assert_eq!(
            read["ReportId"],
            json!(report_id),
            "case {case}: the start and the read must name one report, got {read}"
        );
        assert_report_message(&restarted, true, case);
    }

    // Re-starting over and over converges on the one report rather than accumulating
    // reports in state.
    for round in 1..=8 {
        let again = home.ok("StartQuotaUtilizationReport", json!({})).await;
        assert_eq!(
            again["ReportId"],
            json!(report_id),
            "case re-start {round}: expected the live report {report_id}, got {again}"
        );
        assert_report_message(&again, true, &format!("re-start {round}"));
    }
    let stored = stored_reports(&engine, "us-east-1");
    assert_eq!(
        stored, 1,
        "case eleven starts in one region: expected one stored report, got {stored}"
    );

    // Reuse is per region, like the rest of this service's mutable state: a region
    // holding no live report mints its own.
    let other_region = elsewhere.ok("StartQuotaUtilizationReport", json!({})).await;
    assert_eq!(
        other_region["Status"],
        json!("PENDING"),
        "case another region's first start: it must mint its own report, got {other_region}"
    );
    assert_ne!(
        other_region["ReportId"],
        json!(report_id),
        "case another region's first start: it must not reuse another region's report, \
         got {other_region}"
    );
    assert_report_message(&other_region, false, "another region's first start");

    // Both reports are readable, each at its own endpoint.
    tokio::time::sleep(PAST_COMPLETION).await;
    for (case, caller, id) in [
        ("the first region's report", &home, report_id.clone()),
        (
            "the other region's report",
            &elsewhere,
            other_region["ReportId"]
                .as_str()
                .expect("a minted report reports a ReportId")
                .to_string(),
        ),
    ] {
        let read = caller
            .ok("GetQuotaUtilizationReport", json!({"ReportId": id}))
            .await;
        assert_eq!(
            (read["ReportId"].clone(), read["Status"].clone()),
            (json!(id), json!("COMPLETED")),
            "case {case}: expected a completed report for the id asked for, got {read}"
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
        // publishes, so the record carries the namespace of THAT metric — and a
        // Utilization can only ever accompany one: with no metric there is nothing to
        // count the quota by, so there is no percentage to report.
        let metric_namespace = applied["UsageMetric"]["MetricNamespace"].clone();
        assert_eq!(
            record["Namespace"], metric_namespace,
            "case {case}: Namespace must be the usage metric's namespace, got {record}"
        );
        if metric_namespace.is_null() {
            assert!(
                record["Utilization"].is_null(),
                "case {case}: a quota with no usage metric cannot report a utilization, \
                 got {record}"
            );
        }
        // The usage itself is counted in the services that OWN the resources a quota is
        // counted in, and this suite mounts Service Quotas alone; the formula over an
        // account whose usage was measured is tabled in `quota_utilization_usage`. What
        // holds whatever was counted: a utilization is a percentage of the value in
        // force, so it is never negative.
        if let Some(utilization) = record["Utilization"].as_f64() {
            assert!(
                utilization >= 0.0,
                "case {case}: utilization must be a percentage, got {utilization}"
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

    // Rows: page sizes spanning one record per page, several, the whole report in one
    // page, the modelled maximum (1000) and no MaxResults at all. The whole-report row
    // is bounded by that maximum, because `MaxResultsUtilization` is `@range 1..1000`
    // and a report longer than one page cannot be asked for in one.
    let sizes: Vec<Option<i64>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(whole.len().min(UTILIZATION_REPORT_PAGE_SIZE) as i64),
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
    let home = client(&engine, "us-east-1");
    // A second regional endpoint, because two reports take two regions: a start made
    // where a live report exists reuses it. An increase approved on a quota AWS counts
    // ACCOUNT-WIDE is in force at every endpoint, so one increase is visible to both
    // reports' endpoints and the only difference left between them is WHEN each was
    // started.
    let elsewhere = client(&engine, "eu-west-1");

    // The quota comes out of the catalog rather than being named here: every listed
    // quota reports `GlobalQuota` and `Adjustable`, which is what this row needs.
    let listed = walk_pages(
        &home,
        "ListServiceQuotas",
        &json!({"ServiceCode": "iam"}),
        "Quotas",
        None,
    )
    .await;
    let quota = listed
        .iter()
        .find(|quota| quota["GlobalQuota"] == json!(true) && quota["Adjustable"] == json!(true))
        .expect("the seeded catalog holds an adjustable account-wide quota");
    let service_code = quota["ServiceCode"]
        .as_str()
        .expect("a listed quota reports its ServiceCode")
        .to_string();
    let quota_code = quota["QuotaCode"]
        .as_str()
        .expect("a listed quota reports its QuotaCode")
        .to_string();
    let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
    let key = (service_code.clone(), quota_code.clone());
    let before_value = quota["Value"]
        .as_f64()
        .expect("a listed quota reports its Value");

    // Started BEFORE the increase, at the endpoint that does not request it.
    let before = start_report(&elsewhere).await;

    let desired = before_value + 15.0;
    home.ok(
        "RequestServiceQuotaIncrease",
        json!({
            "ServiceCode": service_code,
            "QuotaCode": quota_code,
            "DesiredValue": desired,
        }),
    )
    .await;
    tokio::time::sleep(PAST_DECISION).await;
    // An account-wide quota's approved value is in force at every endpoint, so both
    // report endpoints see the increase before the next report is started.
    for (case, caller) in [
        ("the requesting region", &home),
        ("another region", &elsewhere),
    ] {
        assert_eq!(
            caller.ok("GetServiceQuota", pair.clone()).await["Quota"]["Value"].as_f64(),
            Some(desired),
            "case {case}: the approved value of an account-wide quota must be in force \
             before the next report"
        );
    }

    let after = start_report(&home).await;
    assert_ne!(
        before, after,
        "case two endpoints: each region's first start mints its own report, got \
         {before} twice"
    );
    tokio::time::sleep(PAST_COMPLETION).await;

    // Rows: the report started before the increase, and the one started after it.
    for (case, caller, report_id, expected) in [
        (
            "started before the increase",
            &elsewhere,
            &before,
            before_value,
        ),
        ("started after the increase", &home, &after, desired),
    ] {
        let records = walk_report(caller, report_id, Some(4)).await;
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
    // Two DIFFERENT reports take two regional endpoints: a second start at one
    // endpoint reuses that region's live report rather than minting another.
    let other = start_report(&elsewhere).await;
    tokio::time::sleep(PAST_COMPLETION).await;
    let token = elsewhere
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

/// Starting reports in a loop does not inflate the store: each region holds the one
/// live report a start reuses, so no start is ever refused for a ceiling.
#[tokio::test]
async fn starting_reports_in_a_loop_neither_inflates_the_store_nor_is_refused() {
    let engine = emulator();
    // Rows: several regional endpoints, each started on many times over.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let starts_per_region = 60;

    for region in regions {
        let caller = client(&engine, region);
        let first = start_report(&caller).await;
        for round in 2..=starts_per_region {
            let started = caller.ok("StartQuotaUtilizationReport", json!({})).await;
            assert_eq!(
                started["ReportId"],
                json!(first),
                "case start {round} in {region}: expected the live report {first}, got \
                 {started}"
            );
        }
        let stored = stored_reports(&engine, region);
        assert_eq!(
            stored, 1,
            "case {starts_per_region} starts in {region}: expected one stored report, got \
             {stored}"
        );
        let read = caller
            .ok("GetQuotaUtilizationReport", json!({"ReportId": first}))
            .await;
        assert_eq!(
            read["ReportId"],
            json!(first),
            "case {region}: the live report must stay readable, got {read}"
        );
    }
}

