// SPDX-License-Identifier: CC0-1.0

//! A quota increase decision is in force from the instant the clock makes it due, at
//! EVERY endpoint of the account — including one that has never been called.
//!
//! The rule under test, over a TABLE of quotas and region pairs rather than one
//! recorded call: the steps a stored quota increase request owes are due at its own
//! instants whether or not anybody calls, and the value an approved increase puts in
//! force reaches as far as the quota's own scope — account-wide for a quota the
//! catalog publishes with `GlobalQuota` true (whose ARN carries no region), this
//! region for a regional one. So a read is answered from the value in force at the
//! instant of the call, never from a value left unmaterialised because the request
//! was made at a regional endpoint nobody has called since.
//!
//! Two consequences each row checks:
//!
//! * the FIRST call made anywhere after the decision comes due already reports the
//!   value in force, even when it is made at an endpoint in a different region from
//!   the one the increase was requested at;
//! * the answer does not change afterwards because of traffic to an unrelated
//!   endpoint: reading, requesting at the requesting region, and reading again all
//!   report the same value.
//!
//! The harness (`Client`, `client`, `emulator`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    REQUEST_DECISION_DELAY, STATUS_PENDING, SUPPORT_CASE_OPEN_DELAY,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// Comfortably past BOTH steps the clock owes a quota increase request that allows a
/// support case: the case opens at `SUPPORT_CASE_OPEN_DELAY`, and the decision the
/// case reviews falls due `REQUEST_DECISION_DELAY` after that.
fn past_decision() -> Duration {
    SUPPORT_CASE_OPEN_DELAY + REQUEST_DECISION_DELAY + Duration::from_millis(600)
}

/// One row: a quota to raise, the endpoint the increase is requested at, and the
/// endpoint that reads it — which never receives a call until after the decision is
/// due.
struct Row {
    /// Why this row is in the table.
    why: &'static str,
    service_code: &'static str,
    /// The quota to raise, by the `QuotaName` the catalog publishes: the `QuotaCode`
    /// and the AWS default value are looked up through
    /// `ListAWSDefaultServiceQuotas`, so no row pins a code or a value of its own.
    quota_name: &'static str,
    /// Whether the catalog publishes this quota as counted account-wide. Asserted
    /// against the catalog's own `GlobalQuota`, so a row can never claim a scope the
    /// service does not report.
    global: bool,
    /// The region the increase is requested at.
    requested_at: &'static str,
    /// The region it is read from, which must differ from `requested_at`.
    read_from: &'static str,
}

/// The table: account-wide quotas of several services, raised in one region and read
/// in another, plus regional quotas as the negative control.
///
/// The quota names are the catalog's (`ListServices` -> `ListAWSDefaultServiceQuotas`),
/// and the regions are codes from the service's own known-region list, which is what
/// bounds its per-region state. Every row raises a quota the issue did not cite.
fn rows() -> Vec<Row> {
    vec![
        Row {
            why: "an account-wide quota of a global service, raised in the US and read in Asia",
            service_code: "iam",
            quota_name: "Roles per account",
            global: true,
            requested_at: "us-east-1",
            read_from: "ap-southeast-2",
        },
        Row {
            why: "an account-wide quota raised in Europe and read in the US",
            service_code: "route53",
            quota_name: "Hosted zones",
            global: true,
            requested_at: "eu-west-1",
            read_from: "us-west-2",
        },
        Row {
            why: "an account-wide quota raised and read in two regions of one continent",
            service_code: "cloudfront",
            quota_name: "Web distributions per account",
            global: true,
            requested_at: "eu-central-1",
            read_from: "eu-north-1",
        },
        Row {
            why: "an account-wide quota whose default is small",
            service_code: "organizations",
            quota_name: "Default maximum number of accounts",
            global: true,
            requested_at: "sa-east-1",
            read_from: "us-east-2",
        },
        Row {
            why: "a REGIONAL quota: the control, in force only where it was requested",
            service_code: "s3",
            quota_name: "General purpose buckets",
            global: false,
            requested_at: "us-east-1",
            read_from: "ap-northeast-1",
        },
        Row {
            why: "a REGIONAL quota of another service: the control, raised elsewhere",
            service_code: "vpc",
            quota_name: "VPCs per Region",
            global: false,
            requested_at: "eu-west-2",
            read_from: "us-west-1",
        },
    ]
}

/// The catalog entry of one quota, by the name it publishes: its `QuotaCode`, its AWS
/// default `Value` and its `GlobalQuota`.
///
/// Read through `ListAWSDefaultServiceQuotas`, which reports the immutable defaults
/// and settles nothing, so looking a row up cannot be what puts a decision in force.
async fn catalog_entry(caller: &Client, service_code: &str, quota_name: &str) -> (String, f64, bool) {
    let listed = walk_pages(
        caller,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await;
    let entry = listed
        .iter()
        .find(|quota| quota["QuotaName"] == json!(quota_name))
        .unwrap_or_else(|| {
            panic!("case {service_code}: the catalog must publish a quota named {quota_name:?}")
        });
    let code = entry["QuotaCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {service_code}/{quota_name}: expected a QuotaCode, got {entry}"))
        .to_string();
    let value = entry["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {service_code}/{quota_name}: expected a Value, got {entry}"));
    let global = entry["GlobalQuota"].as_bool().unwrap_or(false);
    (code, value, global)
}

/// The `Value` `GetServiceQuota` reports for one pair at `caller`'s endpoint.
async fn value_in_force(caller: &Client, service_code: &str, quota_code: &str) -> f64 {
    let quota = caller
        .ok(
            "GetServiceQuota",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        )
        .await;
    quota["Quota"]["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {service_code}/{quota_code}: expected a Value, got {quota}"))
}

/// The `Value` `ListServiceQuotas` reports for the same pair, so the listing and the
/// single read are checked to agree on the value in force.
async fn listed_value(caller: &Client, service_code: &str, quota_code: &str) -> f64 {
    let listed = walk_pages(
        caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await;
    let entry = listed
        .iter()
        .find(|quota| quota["QuotaCode"] == json!(quota_code))
        .unwrap_or_else(|| panic!("case {service_code}/{quota_code}: the listing must carry the pair"));
    entry["Value"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {service_code}/{quota_code}: expected a Value, got {entry}"))
}

/// Request an increase and return the `QuotaCode` it was made for, asserting it was
/// accepted `PENDING`.
async fn request_increase(caller: &Client, service_code: &str, quota_code: &str, desired: f64) {
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
    assert_eq!(
        response["RequestedQuota"]["Status"].as_str(),
        Some(STATUS_PENDING),
        "case {service_code}/{quota_code}: expected an accepted request, got {response}"
    );
}

/// The first call made anywhere after the decision comes due reports the value in
/// force, and no later call to another endpoint changes it.
#[tokio::test]
async fn a_due_decision_is_in_force_at_an_endpoint_that_has_not_been_called() {
    for row in rows() {
        assert_ne!(
            row.requested_at, row.read_from,
            "case {}: the reading endpoint must be in another region",
            row.why
        );

        // A fresh emulator per row, so the only traffic the reading endpoint has seen
        // is this row's.
        let engine = emulator();
        let requester = client(&engine, row.requested_at);
        let reader = client(&engine, row.read_from);

        let (quota_code, default_value, global) =
            catalog_entry(&reader, row.service_code, row.quota_name).await;
        assert_eq!(
            global, row.global,
            "case {}: the catalog must publish GlobalQuota {} for {}/{quota_code}",
            row.why, row.global, row.service_code
        );

        let desired = default_value * 2.0;
        request_increase(&requester, row.service_code, &quota_code, desired).await;

        tokio::time::sleep(past_decision()).await;

        // The value the quota's scope puts in force at the READING endpoint: the
        // raised one for an account-wide quota, the untouched default for a regional
        // one whose increase was requested elsewhere.
        let expected = if row.global { desired } else { default_value };

        // The FIRST call made after the decision came due, and it is at the endpoint
        // the increase was NOT requested at.
        let first = value_in_force(&reader, row.service_code, &quota_code).await;
        assert_eq!(
            first, expected,
            "case {}: the first read at {} must report {expected}, got {first}",
            row.why, row.read_from
        );

        let listed = listed_value(&reader, row.service_code, &quota_code).await;
        assert_eq!(
            listed, expected,
            "case {}: the listing at {} must agree with the single read ({expected}), got {listed}",
            row.why, row.read_from
        );

        // The requesting endpoint reports the raised value for either scope, since it
        // is where the increase was applied.
        let at_requester = value_in_force(&requester, row.service_code, &quota_code).await;
        assert_eq!(
            at_requester, desired,
            "case {}: the read at {} must report the raised value {desired}, got {at_requester}",
            row.why, row.requested_at
        );

        // And a call to that unrelated endpoint changed nothing here.
        let again = value_in_force(&reader, row.service_code, &quota_code).await;
        assert_eq!(
            again, first,
            "case {}: the read at {} must not change because {} was called, got {again} after \
             {first}",
            row.why, row.read_from, row.requested_at
        );

        // The AWS default is untouched at both endpoints, whatever is in force.
        for (caller, region) in [(&requester, row.requested_at), (&reader, row.read_from)] {
            let (_, still_default, _) =
                catalog_entry(caller, row.service_code, row.quota_name).await;
            assert_eq!(
                still_default, default_value,
                "case {}: the AWS default at {region} must stay {default_value}, got \
                 {still_default}",
                row.why
            );
        }
    }
}

/// The value in force account-wide is what a FURTHER request is measured against, at
/// an endpoint that has not been called: asking again for the value already in force
/// is not an increase, and is refused.
#[tokio::test]
async fn a_request_at_an_uncalled_endpoint_is_measured_against_the_value_in_force() {
    for row in rows().into_iter().filter(|row| row.global) {
        let engine = emulator();
        let requester = client(&engine, row.requested_at);
        let reader = client(&engine, row.read_from);

        let (quota_code, default_value, _) =
            catalog_entry(&reader, row.service_code, row.quota_name).await;
        let desired = default_value * 2.0;
        request_increase(&requester, row.service_code, &quota_code, desired).await;

        tokio::time::sleep(past_decision()).await;

        // The first call at the other endpoint after the decision came due asks for
        // the value already in force account-wide, so it is not an increase.
        let (status, code) = reader
            .refused(
                "RequestServiceQuotaIncrease",
                json!({
                    "ServiceCode": row.service_code,
                    "QuotaCode": quota_code,
                    "DesiredValue": desired,
                }),
            )
            .await;
        assert_eq!(
            (status, code.contains("IllegalArgumentException")),
            (400, true),
            "case {}: asking {} again for the in-force value {desired} must be refused, got \
             {status} {code}",
            row.why,
            row.read_from
        );

        // A value above it still is an increase, and lands.
        let higher = desired + default_value;
        request_increase(&reader, row.service_code, &quota_code, higher).await;
        tokio::time::sleep(past_decision()).await;
        for (caller, region) in [(&requester, row.requested_at), (&reader, row.read_from)] {
            let now_in_force = value_in_force(caller, row.service_code, &quota_code).await;
            assert_eq!(
                now_in_force, higher,
                "case {}: an account-wide quota must report {higher} at {region}, got \
                 {now_in_force}",
                row.why
            );
        }
    }
}

/// `ListServiceQuotas` reports every quota from the value in force, so a row never
/// drifts from what `GetServiceQuota` answers for the same pair — at an endpoint the
/// increase was not requested at.
#[tokio::test]
async fn a_listing_at_an_uncalled_endpoint_agrees_with_the_single_read() {
    let engine = emulator();
    let requester = client(&engine, "us-east-1");
    let reader = client(&engine, "ap-south-1");

    // Every account-wide row of one global service is raised at once, so the listing
    // is checked over several rows of the same page rather than one.
    let listed = walk_pages(
        &requester,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": "iam"}),
        "Quotas",
        None,
    )
    .await;
    let mut raised: Vec<(String, f64)> = Vec::new();
    for quota in &listed {
        let code = quota["QuotaCode"].as_str().expect("a QuotaCode").to_string();
        let value = quota["Value"].as_f64().expect("a Value");
        if quota["Adjustable"] != json!(true) {
            continue;
        }
        let desired = value * 3.0;
        request_increase(&requester, "iam", &code, desired).await;
        raised.push((code, desired));
    }
    assert!(
        raised.len() > 1,
        "case the iam catalog: expected several adjustable account-wide quotas, got {}",
        raised.len()
    );

    tokio::time::sleep(past_decision()).await;

    // The reading endpoint's first call is the LISTING, which must already carry
    // every raised value.
    let after = walk_pages(
        &reader,
        "ListServiceQuotas",
        &json!({"ServiceCode": "iam"}),
        "Quotas",
        None,
    )
    .await;
    for (code, desired) in &raised {
        let entry = after
            .iter()
            .find(|quota| quota["QuotaCode"] == json!(code))
            .unwrap_or_else(|| panic!("case iam/{code}: the listing must carry the pair"));
        assert_eq!(
            entry["Value"].as_f64(),
            Some(*desired),
            "case iam/{code} listed at ap-south-1: expected {desired}, got {entry}"
        );
        let read = value_in_force(&reader, "iam", code).await;
        assert_eq!(
            read, *desired,
            "case iam/{code} read at ap-south-1: expected {desired}, got {read}"
        );
    }
}
