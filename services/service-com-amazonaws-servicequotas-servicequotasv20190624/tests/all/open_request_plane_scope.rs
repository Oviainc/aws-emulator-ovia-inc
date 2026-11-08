// SPDX-License-Identifier: CC0-1.0

//! The "one request awaiting a decision" rule holds over a quota PLANE, and a plane is
//! as wide as the value an approved increase puts in force.
//!
//! The rule under test, over a TABLE of quotas and region pairs rather than one
//! recorded call: a quota the catalog publishes as counted account-wide
//! (`GlobalQuota` true) has ONE value for the whole account, so a request awaiting a
//! decision on it blocks a further request made at ANY regional endpoint
//! (`ResourceAlreadyExistsException`, HTTP 400); a quota counted per region has one
//! value per region, so each regional endpoint carries its own open request. Two
//! consequences every row checks:
//!
//! * an increase reported `APPROVED` is the value in force afterwards — no second
//!   approval on the same plane can have overwritten it, because no second request on
//!   it was ever accepted;
//! * the refusal is about the PLANE, not the endpoint: another quota of the same
//!   service is still accepted at the blocked endpoint, and once the open request is
//!   decided the plane takes a further increase at either endpoint.
//!
//! The harness (`Client`, `client`, `emulator`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::ONE_OPEN_REQUEST_MESSAGE;
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    REQUEST_DECISION_DELAY, STATUS_APPROVED, STATUS_PENDING, SUPPORT_CASE_OPEN_DELAY,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request that allows a
/// support case: the case opens at `SUPPORT_CASE_OPEN_DELAY`, and the decision the
/// case reviews falls due `REQUEST_DECISION_DELAY` after that.
///
/// `pub(crate)` so the suites that share this file's helpers wait by the same span.
pub(crate) fn past_decision() -> Duration {
    SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY + Duration::from_millis(600)
}

/// One row: a quota, the endpoint the first increase is requested at, and a second
/// endpoint in another region that then asks for the same quota.
struct Row {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    /// The quota, by the `QuotaName` the catalog publishes: its `QuotaCode`, AWS
    /// default `Value` and `GlobalQuota` are read through `ListAWSDefaultServiceQuotas`,
    /// so no row pins a code or a value of its own.
    quota_name: &'static str,
    /// Whether the catalog publishes this quota as counted account-wide. Asserted
    /// against the catalog's own `GlobalQuota`, so a row can never claim a scope the
    /// service does not report.
    global: bool,
    /// The endpoint the first increase is requested at.
    first_at: &'static str,
    /// A second endpoint, in another region, which asks for the same quota while the
    /// first request is still awaiting a decision.
    second_at: &'static str,
}

/// The table: account-wide quotas of several services, in several region pairs and
/// over a wide spread of default values, plus per-region quotas as the control.
///
/// The quota names are the catalog's (`ListServices` ->
/// `ListAWSDefaultServiceQuotas`) and every row names a quota the issue did not cite.
fn rows() -> Vec<Row> {
    vec![
        Row {
            why: "an account-wide quota of a global service, asked for in the US then in Europe",
            service_code: "iam",
            quota_name: "Users per account",
            global: true,
            first_at: "us-east-1",
            second_at: "eu-west-1",
        },
        Row {
            why: "an account-wide quota asked for in Europe then in the US",
            service_code: "route53",
            quota_name: "Records per hosted zone",
            global: true,
            first_at: "eu-west-1",
            second_at: "us-east-1",
        },
        Row {
            why: "an account-wide quota whose second endpoint sorts BEFORE the first",
            service_code: "cloudfront",
            quota_name: "Cache behaviors per distribution",
            global: true,
            first_at: "us-west-2",
            second_at: "ap-northeast-1",
        },
        Row {
            why: "an account-wide quota with a small default, over two southern regions",
            service_code: "organizations",
            quota_name: "Default maximum number of accounts",
            global: true,
            first_at: "sa-east-1",
            second_at: "ap-southeast-2",
        },
        Row {
            why: "an account-wide quota of a service that publishes a single quota",
            service_code: "globalaccelerator",
            quota_name: "Accelerators per account",
            global: true,
            first_at: "eu-central-1",
            second_at: "us-east-2",
        },
        Row {
            why: "a PER-REGION quota: the control, one open request per region",
            service_code: "vpc",
            quota_name: "VPCs per Region",
            global: false,
            first_at: "us-east-1",
            second_at: "eu-west-1",
        },
        Row {
            why: "a PER-REGION quota of another service: the control, raised elsewhere",
            service_code: "lambda",
            quota_name: "Concurrent executions",
            global: false,
            first_at: "ap-south-1",
            second_at: "us-west-1",
        },
    ]
}

/// Every quota of one service as `ListAWSDefaultServiceQuotas` publishes it.
async fn catalog(caller: &Client, service_code: &str) -> Vec<Value> {
    walk_pages(
        caller,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
}

/// The catalog entry of one quota, by the name it publishes: its `QuotaCode`, its AWS
/// default `Value` and its `GlobalQuota`.
///
/// `pub(crate)` so another suite reads the catalog through this one lookup rather than
/// growing a copy of it.
pub(crate) async fn catalog_entry(
    caller: &Client,
    service_code: &str,
    quota_name: &str,
) -> (String, f64, bool) {
    let listed = catalog(caller, service_code).await;
    let entry = listed
        .iter()
        .find(|quota| quota["QuotaName"] == json!(quota_name))
        .unwrap_or_else(|| {
            panic!("case {service_code}: the catalog must publish a quota named {quota_name:?}")
        });
    let code = entry["QuotaCode"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {service_code}/{quota_name}: expected a QuotaCode, got {entry}")
        })
        .to_string();
    let value = entry["Value"].as_f64().unwrap_or_else(|| {
        panic!("case {service_code}/{quota_name}: expected a Value, got {entry}")
    });
    let global = entry["GlobalQuota"].as_bool().unwrap_or(false);
    (code, value, global)
}

/// Another adjustable quota of the same service, so the refusal can be shown to be
/// about the plane rather than the endpoint. `None` for a service that publishes only
/// the quota under test.
async fn another_adjustable_quota(
    caller: &Client,
    service_code: &str,
    besides: &str,
) -> Option<(String, f64)> {
    catalog(caller, service_code).await.iter().find_map(|quota| {
        let code = quota["QuotaCode"].as_str()?;
        if code == besides || quota["Adjustable"] != json!(true) {
            return None;
        }
        Some((code.to_string(), quota["Value"].as_f64()?))
    })
}

/// Request an increase that must be accepted, and return the stored record.
///
/// `pub(crate)`: shared with the suite that reads a minted request back.
pub(crate) async fn accepted(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    desired: f64,
) -> Value {
    let response = caller
        .ok(
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": desired,
            }),
        )
        .await;
    let request = response["RequestedQuota"].clone();
    assert_eq!(
        request["Status"].as_str(),
        Some(STATUS_PENDING),
        "case {service_code}/{quota_code} at {desired}: expected an accepted PENDING request, \
         got {response}"
    );
    request
}

/// The status and error code of a request that must be refused, with the refusal's
/// message member.
///
/// `pub(crate)`: shared with the suite that reads the request a refusal names.
pub(crate) async fn refusal(
    caller: &Client,
    service_code: &str,
    quota_code: &str,
    desired: f64,
) -> (u16, String, String) {
    let (status, body) = caller
        .call(
            "RequestServiceQuotaIncrease",
            &json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": desired,
            }),
        )
        .await;
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string();
    let message = body
        .get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// The `Value` `GetServiceQuota` reports for one pair at `caller`'s endpoint.
async fn value_in_force(caller: &Client, service_code: &str, quota_code: &str) -> f64 {
    let quota = caller
        .ok(
            "GetServiceQuota",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        )
        .await;
    quota["Quota"]["Value"].as_f64().unwrap_or_else(|| {
        panic!("case {service_code}/{quota_code}: expected a Value, got {quota}")
    })
}

/// The `Status` and `DesiredValue` `GetRequestedServiceQuotaChange` reports for one
/// stored request at `caller`'s endpoint.
async fn stored_request(caller: &Client, id: &str) -> (String, f64) {
    let response = caller
        .ok("GetRequestedServiceQuotaChange", json!({"RequestId": id}))
        .await;
    let request = &response["RequestedQuota"];
    let status = request["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case request {id}: expected a Status, got {response}"))
        .to_string();
    let desired = request["DesiredValue"]
        .as_f64()
        .unwrap_or_else(|| panic!("case request {id}: expected a DesiredValue, got {response}"));
    (status, desired)
}

/// A request awaiting a decision on an account-wide quota blocks a request for it at
/// EVERY endpoint; one on a per-region quota blocks only its own region.
#[tokio::test]
async fn an_open_request_blocks_the_whole_plane_it_was_made_on() {
    for row in rows() {
        assert_ne!(
            row.first_at, row.second_at,
            "case {}: the second endpoint must be in another region",
            row.why
        );

        // A fresh emulator per row, so every open request in play is this row's.
        let engine = emulator();
        let first_caller = client(&engine, row.first_at);
        let second_caller = client(&engine, row.second_at);

        let (quota_code, default_value, global) =
            catalog_entry(&first_caller, row.service_code, row.quota_name).await;
        assert_eq!(
            global, row.global,
            "case {}: the catalog must publish GlobalQuota {} for {}/{quota_code}",
            row.why, row.global, row.service_code
        );

        let first_desired = default_value * 2.0;
        let first = accepted(&first_caller, row.service_code, &quota_code, first_desired).await;
        let first_id = first["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected an Id, got {first}", row.why))
            .to_string();

        // The second endpoint asks for the same quota while the first request is still
        // awaiting a decision.
        let second_desired = default_value * 3.0;
        if row.global {
            let (status, code, message) =
                refusal(&second_caller, row.service_code, &quota_code, second_desired).await;
            assert_eq!(
                (status, code.contains("ResourceAlreadyExistsException")),
                (400, true),
                "case {}: a second open request on one account-wide plane must be refused at \
                 {}, got {status} {code}",
                row.why,
                row.second_at
            );
            assert_eq!(
                message,
                ONE_OPEN_REQUEST_MESSAGE,
                "case {}: the refusal must state the service's fixed sentence for the \
                 fact, got {message:?}",
                row.why
            );
            assert!(
                !message.contains(&first_id),
                "case {}: the refusal must not name the open request {first_id}, got \
                 {message:?}",
                row.why
            );
        } else {
            let second =
                accepted(&second_caller, row.service_code, &quota_code, second_desired).await;
            assert_ne!(
                second["Id"].as_str(),
                Some(first_id.as_str()),
                "case {}: each region's plane must mint its own request, got {second}",
                row.why
            );
        }

        // Either way the refusal is about the PLANE: another quota of the same service
        // is still accepted at the second endpoint.
        if let Some((other_code, other_default)) =
            another_adjustable_quota(&second_caller, row.service_code, &quota_code).await
        {
            accepted(
                &second_caller,
                row.service_code,
                &other_code,
                other_default * 2.0,
            )
            .await;
        }

        tokio::time::sleep(past_decision()).await;

        // The first request is APPROVED and its value is what is in force — at both
        // endpoints for an account-wide quota, and at its own endpoint for a
        // per-region one, where the second region holds its own value.
        let (status, desired) = stored_request(&first_caller, &first_id).await;
        assert_eq!(
            (status.as_str(), desired),
            (STATUS_APPROVED, first_desired),
            "case {}: the first request must be APPROVED at {first_desired}, got {status} \
             {desired}",
            row.why
        );
        let expectations = if row.global {
            [
                (row.first_at, first_desired),
                (row.second_at, first_desired),
            ]
        } else {
            [
                (row.first_at, first_desired),
                (row.second_at, second_desired),
            ]
        };
        for (caller, (region, expected)) in
            [&first_caller, &second_caller].into_iter().zip(expectations)
        {
            let in_force = value_in_force(caller, row.service_code, &quota_code).await;
            assert_eq!(
                in_force, expected,
                "case {}: the APPROVED increase must be in force at {region} as {expected}, got \
                 {in_force}",
                row.why
            );
        }
    }
}

/// With the plane's open request decided, the plane takes a further increase — made at
/// the OTHER endpoint — and that one is in force in turn, whichever endpoint reads it.
#[tokio::test]
async fn a_decided_plane_takes_the_next_increase_at_either_endpoint() {
    for row in rows() {
        let engine = emulator();
        let first_caller = client(&engine, row.first_at);
        let second_caller = client(&engine, row.second_at);

        let (quota_code, default_value, _) =
            catalog_entry(&first_caller, row.service_code, row.quota_name).await;

        let first_desired = default_value * 2.0;
        accepted(&first_caller, row.service_code, &quota_code, first_desired).await;
        tokio::time::sleep(past_decision()).await;

        // Now that nothing is awaiting a decision on the plane, the other endpoint's
        // request is accepted — and for an account-wide quota it must be measured
        // against the value the first increase put in force everywhere.
        let next_desired = first_desired * 2.0;
        let next = accepted(&second_caller, row.service_code, &quota_code, next_desired).await;
        let next_id = next["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: expected an Id, got {next}", row.why))
            .to_string();

        // While THAT one is open, the plane is blocked again — at the first endpoint
        // for an account-wide quota, which is the endpoint that owns the earlier,
        // already decided request.
        if row.global {
            let (status, code, _) = refusal(
                &first_caller,
                row.service_code,
                &quota_code,
                next_desired + default_value,
            )
            .await;
            assert_eq!(
                (status, code.contains("ResourceAlreadyExistsException")),
                (400, true),
                "case {}: the plane must be blocked again at {}, got {status} {code}",
                row.why,
                row.first_at
            );
        }

        tokio::time::sleep(past_decision()).await;

        let (status, desired) = stored_request(&second_caller, &next_id).await;
        assert_eq!(
            (status.as_str(), desired),
            (STATUS_APPROVED, next_desired),
            "case {}: the second increase must be APPROVED at {next_desired}, got {status} \
             {desired}",
            row.why
        );

        // The LAST decision is the one in force on the plane: account-wide at both
        // endpoints, and per region for a quota counted per region (where the first
        // endpoint keeps its own earlier value).
        let expected_at_first = if row.global {
            next_desired
        } else {
            first_desired
        };
        for (caller, region, expected) in [
            (&first_caller, row.first_at, expected_at_first),
            (&second_caller, row.second_at, next_desired),
        ] {
            let in_force = value_in_force(caller, row.service_code, &quota_code).await;
            assert_eq!(
                in_force, expected,
                "case {}: {region} must report {expected} after the second decision, got \
                 {in_force}",
                row.why
            );
        }
    }
}
