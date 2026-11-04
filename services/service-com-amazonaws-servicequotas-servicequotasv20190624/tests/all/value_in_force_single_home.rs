// SPDX-License-Identifier: CC0-1.0

//! One value in force per quota, whichever operation reports it.
//!
//! The rule both the quota reads and a quota utilization report resolve a quota's
//! `Value` by: the value in force for a quota is the applied overlay a decided
//! increase request wrote for the plane being read, else the AWS default — resolved
//! once, in this crate's single `value_in_force` helper, so every reader answers with
//! the same number.
//!
//! This suite drives that one path through an operation on each side of it:
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` (which measures a `DesiredValue`
//! against the value in force and names it in its refusal), and
//! `StartQuotaUtilizationReport` / `GetQuotaUtilizationReport`, whose every record
//! carries the same value as `AppliedValue`.
//!
//! The rows span what the rule resolves over: a quota AWS counts account-wide and a
//! quota it counts per Region, each raised by an approved increase; an adjustable quota
//! no increase was approved for; a quota no increase can be requested for at all; and
//! each of them read BOTH at the endpoint the increases were requested at and at one
//! that was never called — where a per-region quota has no overlay and the AWS default
//! is still in force.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::NOT_ABOVE_CURRENT_VALUE_MESSAGE;

// The wire harness this crate already has: `POST /` with `X-Amz-Target:
// ServiceQuotasV20190624.<Op>` through the `MultiServiceRouter`, over one shared
// engine, plus its listing walker. Declared `pub(crate)` in `quota_workflow` for
// sibling suites to reuse.
use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request
/// (`crate::state::SUPPORT_CASE_OPEN_DELAY`, then
/// `crate::state::REQUEST_DECISION_DELAY` once its support case is open).
const PAST_DECISION: Duration = Duration::from_millis(1_800);

/// Comfortably past `crate::state::REPORT_COMPLETION_DELAY` (500 ms), so a report
/// started before this wait has completed afterwards.
const PAST_COMPLETION: Duration = Duration::from_millis(1_200);

/// How much an approved increase raises a quota by, so the value in force differs from
/// the AWS default by a number no catalog row carries by accident.
const RAISE: f64 = 25.0;

/// The `(ServiceCode, QuotaCode)` key of a quota record, the key every reader of a
/// quota reports it under.
fn key_of(quota: &Value) -> (String, String) {
    (
        quota["ServiceCode"].as_str().unwrap_or_default().to_string(),
        quota["QuotaCode"].as_str().unwrap_or_default().to_string(),
    )
}

/// Every quota in the catalog, as `ListServiceQuotas` reports it.
///
/// Both listings are walked to their end: the catalog is several hundred quotas across
/// several dozen services, so one page is not all of it.
async fn catalog_quotas(client: &Client) -> Vec<Value> {
    let services = walk_pages(client, "ListServices", &json!({}), "Services", None).await;
    let mut quotas = Vec::new();
    for service in &services {
        let service_code = service["ServiceCode"]
            .as_str()
            .expect("a listed service reports its ServiceCode");
        quotas.extend(
            walk_pages(
                client,
                "ListServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                None,
            )
            .await,
        );
    }
    assert!(
        !quotas.is_empty(),
        "the seeded catalog must hold at least one quota"
    );
    quotas
}

/// The first catalog quota the row needs that no earlier row already took, so the rows
/// are distinct quotas and come out of the catalog rather than being named here.
fn pick(
    quotas: &[Value],
    taken: &mut BTreeSet<(String, String)>,
    case: &str,
    wanted: impl Fn(&Value) -> bool,
) -> Value {
    let found = quotas
        .iter()
        .find(|quota| wanted(quota) && !taken.contains(&key_of(quota)))
        .unwrap_or_else(|| panic!("case {case}: the seeded catalog must hold such a quota"))
        .clone();
    taken.insert(key_of(&found));
    found
}

/// One row: a quota out of the catalog, and whether an increase is approved for it.
struct Row {
    case: &'static str,
    quota: Value,
    increase: bool,
}

#[tokio::test]
async fn every_reader_of_a_quota_reports_the_same_value_in_force() {
    let engine = emulator();
    // The endpoint the increases are requested at, and one never called before the
    // reports are started: a quota counted per Region has an overlay only at the first,
    // while an account-wide one is in force at both.
    let home = client(&engine, "us-east-1");
    let elsewhere = client(&engine, "eu-west-1");

    let quotas = catalog_quotas(&home).await;
    let mut taken: BTreeSet<(String, String)> = BTreeSet::new();
    let rows = vec![
        Row {
            case: "an account-wide quota raised by an approved increase",
            quota: pick(
                &quotas,
                &mut taken,
                "an account-wide quota raised by an approved increase",
                |quota| quota["GlobalQuota"] == json!(true) && quota["Adjustable"] == json!(true),
            ),
            increase: true,
        },
        Row {
            case: "a per-region quota raised by an approved increase",
            quota: pick(
                &quotas,
                &mut taken,
                "a per-region quota raised by an approved increase",
                |quota| quota["GlobalQuota"] == json!(false) && quota["Adjustable"] == json!(true),
            ),
            increase: true,
        },
        Row {
            case: "an adjustable quota no increase was approved for",
            quota: pick(
                &quotas,
                &mut taken,
                "an adjustable quota no increase was approved for",
                |quota| quota["Adjustable"] == json!(true),
            ),
            increase: false,
        },
        Row {
            case: "a quota no increase can be requested for",
            quota: pick(
                &quotas,
                &mut taken,
                "a quota no increase can be requested for",
                |quota| quota["Adjustable"] == json!(false),
            ),
            increase: false,
        },
    ];

    // Raise the rows that ask to be raised, at the home endpoint only, and let the
    // clock settle both steps the request owes.
    for row in rows.iter().filter(|row| row.increase) {
        let (service_code, quota_code) = key_of(&row.quota);
        let default_value = row.quota["Value"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: a listed quota reports its Value", row.case));
        home.ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": default_value + RAISE,
            }),
        )
        .await;
    }
    tokio::time::sleep(PAST_DECISION).await;

    // One report per endpoint, started after every increase was decided, so each
    // report's records are the values in force at that endpoint.
    let mut reports = Vec::new();
    for (region, caller) in [("us-east-1", &home), ("eu-west-1", &elsewhere)] {
        let started = caller.ok("StartQuotaUtilizationReport", json!({})).await;
        let report_id = started["ReportId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region}: a start must report a ReportId, got {started}"))
            .to_string();
        reports.push((region, caller, report_id));
    }
    tokio::time::sleep(PAST_COMPLETION).await;

    for (region, caller, report_id) in &reports {
        let records = walk_pages(
            caller,
            "GetQuotaUtilizationReport",
            &json!({"ReportId": report_id}),
            "Quotas",
            Some(50),
        )
        .await;
        let by_pair: BTreeMap<(String, String), Value> = records
            .iter()
            .map(|record| (key_of(record), record.clone()))
            .collect();

        for row in &rows {
            let (service_code, quota_code) = key_of(&row.quota);
            let case = format!("{} read at {region}", row.case);
            let default_value = row.quota["Value"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case}: a listed quota reports its Value"));
            // An overlay written for an account-wide quota is in force at every
            // endpoint; one written for a per-region quota only where it was written.
            let overlay_here = row.increase
                && (row.quota["GlobalQuota"] == json!(true) || *region == "us-east-1");
            let expected = if overlay_here {
                default_value + RAISE
            } else {
                default_value
            };

            let read = caller
                .ok(
                    "GetServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                read["Quota"]["Value"].as_f64(),
                Some(expected),
                "case {case}: expected the value in force {expected}, got {read}"
            );

            let record = by_pair
                .get(&(service_code.clone(), quota_code.clone()))
                .unwrap_or_else(|| {
                    panic!("case {case}: the report must cover {service_code}/{quota_code}")
                });
            assert_eq!(
                record["AppliedValue"].as_f64(),
                Some(expected),
                "case {case}: a report's AppliedValue must be the value in force {expected}, \
                 got {record}"
            );
            assert_eq!(
                record["AppliedValue"].as_f64(),
                read["Quota"]["Value"].as_f64(),
                "case {case}: a report record and the quota read must agree, got {record} \
                 against {read}"
            );
            // An increase never moves the AWS default, so a raised row still shows the
            // two readers resolving DIFFERENT numbers for the same pair.
            assert_eq!(
                record["DefaultValue"].as_f64(),
                Some(default_value),
                "case {case}: DefaultValue must stay the AWS default {default_value}, \
                 got {record}"
            );
        }
    }

    // The third reader of the same rule: a request is measured against the value in
    // force, and asking again for that value is refused — in the service's own fixed
    // sentence for that bound, which names no value at all (`refusal_wording.rs` spans
    // both bounds of this refusal and the order they are checked in).
    for row in rows
        .iter()
        .filter(|row| row.quota["Adjustable"] == json!(true))
    {
        for (region, caller) in [("us-east-1", &home), ("eu-west-1", &elsewhere)] {
            let (service_code, quota_code) = key_of(&row.quota);
            let case = format!("{} measured at {region}", row.case);
            let in_force = caller
                .ok(
                    "GetServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await["Quota"]["Value"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case}: a read reports the value in force"));

            let (status, body) = caller
                .call(
                    "RequestServiceQuotaIncrease",
                    &json!({
                        "ServiceCode": service_code,
                        "QuotaCode": quota_code,
                        "DesiredValue": in_force,
                    }),
                )
                .await;
            assert_eq!(
                (status, body["__type"].as_str().unwrap_or_default().to_string()),
                (400, "IllegalArgumentException".to_string()),
                "case {case}: asking again for the in-force value {in_force} must be refused, \
                 got {status} with {body}"
            );
            let message = body
                .get("Message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a refusal carries a Message, got {body}"));
            assert_eq!(
                message, NOT_ABOVE_CURRENT_VALUE_MESSAGE,
                "case {case}: asking again for the in-force value {in_force} must be refused in \
                 the service's sentence for that bound, got {message}"
            );
        }
    }
}
