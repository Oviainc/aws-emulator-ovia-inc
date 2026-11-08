// SPDX-License-Identifier: CC0-1.0

//! What the applied-quota listing publishes for the API REQUEST-RATE quotas of a
//! service, and the page it publishes them on.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the wire
//! behaviour.
//!
//! Two rules are pinned, each over a CLASS of rows rather than over one quota:
//!
//! * a quota that throttles ONE API operation of this service — to so many requests per
//!   second, or to so many in one burst — is described by that class's sentence around
//!   the operation's own name, so the text is DERIVED here from each row's `QuotaName`
//!   instead of being listed row by row: however many such quotas the catalog publishes,
//!   each one's `Description` has to be its own. The wording is what tells a client a
//!   rate quota from a count of the same size and a burst allowance from the sustained
//!   rate beside it, and a row that borrows another quota's sentence (or a paraphrase of
//!   its own) fails here;
//! * such a quota is reached by the `QuotaCode` AWS publishes it under, through every
//!   read path, and it sits on a page that is BOUNDED: the applied listing's unasked
//!   page (`crate::state::APPLIED_QUOTA_PAGE_SIZE`) is followed by a `NextToken`
//!   whenever the service has quotas past it, and chaining that token reports every
//!   quota of the service exactly once.
//!
//! `applied_quota_records.rs` pins the whole record of individual rows; what this suite
//! adds is the rule over the class and the page the class is served on.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    APPLIED_QUOTA_PAGE_SIZE, MAX_PAGE_SIZE,
};

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// The operation the rules are asserted through.
const OPERATION: &str = "ListServiceQuotas";

/// The service whose published catalog carries API request-rate quotas under AWS's own
/// `QuotaCode`s, which is what makes it the service the rate rule is asserted over.
const RATE_SERVICE: &str = "lambda";

/// How many rows of the per-second rate class the catalog must publish for the rule to
/// have been asserted over a class rather than over one row.
const RATE_ROWS_EXPECTED: usize = 3;

/// The API operation a `QuotaName` of the per-second rate class throttles, or `None`
/// for a name outside that class.
///
/// The class is `Rate of <Api> API requests` where `<Api>` is ONE API operation's own
/// name — a single word in the casing an operation is named in. A name whose middle is
/// prose ("control plane", "capacity provider write") describes a rate over a GROUP of
/// operations and publishes its own sentence about that group, so it is not of this
/// class.
fn rated_api(quota_name: &str) -> Option<&str> {
    let api = quota_name
        .strip_prefix("Rate of ")?
        .strip_suffix(" API requests")?;
    let names_one_operation = !api.is_empty()
        && !api.contains(char::is_whitespace)
        && api.starts_with(char::is_uppercase);
    names_one_operation.then_some(api)
}

/// The API operation a `QuotaName` of the BURST class names, or `None`.
///
/// The same reading [`rated_api`] takes of the sustained class, applied to
/// `Burst rate of <Api> API requests`: the middle has to name ONE API operation.
fn bursted_api(quota_name: &str) -> Option<&str> {
    let api = quota_name
        .strip_prefix("Burst rate of ")?
        .strip_suffix(" API requests")?;
    let names_one_operation = !api.is_empty()
        && !api.contains(char::is_whitespace)
        && api.starts_with(char::is_uppercase);
    names_one_operation.then_some(api)
}

/// The applied-quota listing of `service_code`, walked with the tokens the service
/// issues at the largest page a caller may ask for.
async fn applied_listing(caller: &Client, service_code: &str) -> Vec<Value> {
    walk_pages(
        caller,
        OPERATION,
        &json!({"ServiceCode": service_code}),
        "Quotas",
        Some(MAX_PAGE_SIZE as i64),
    )
    .await
}

/// RULE: a quota that throttles one API operation to so many requests per second
/// publishes the per-second sentence of THAT operation — derived from the row's own
/// `QuotaName`, so every row of the class is a case.
#[tokio::test]
async fn an_api_rate_quota_describes_its_own_operation_per_second() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let listed = applied_listing(&caller, RATE_SERVICE).await;
    let mut rate_rows = 0_usize;
    let mut burst_rows = 0_usize;

    for quota in &listed {
        let name = quota["QuotaName"].as_str().unwrap_or_default();
        let code = quota["QuotaCode"].as_str().unwrap_or_default();
        let described = quota["Description"].as_str();

        if let Some(api) = rated_api(name) {
            rate_rows += 1;
            let expected = format!("The maximum number of {api} API requests per second.");
            assert_eq!(
                described,
                Some(expected.as_str()),
                "case {RATE_SERVICE}/{code} ({name:?}): expected the per-second sentence \
                 {expected:?}, got {described:?}"
            );
            continue;
        }

        if let Some(api) = bursted_api(name) {
            burst_rows += 1;
            // The burst allowance beside such a rate: the sentence of ITS operation,
            // naming the burst rather than the steady rate, so a paraphrase of it and a
            // neighbouring quota's text both fail.
            let expected =
                format!("The maximum number of {api} API requests that can be sent in one burst.");
            assert_eq!(
                described,
                Some(expected.as_str()),
                "case {RATE_SERVICE}/{code} ({name:?}): expected the burst sentence {expected:?}, \
                 got {described:?}"
            );
            continue;
        }

        // Every other row: its description must not be the per-second sentence of an
        // operation it does not throttle, which is the shape of the defect.
        if let Some(described) = described {
            for other in &listed {
                let Some(api) = other["QuotaName"].as_str().and_then(rated_api) else {
                    continue;
                };
                let borrowed = format!("The maximum number of {api} API requests per second.");
                assert_ne!(
                    described, borrowed,
                    "case {RATE_SERVICE}/{code} ({name:?}): a row must not publish another \
                     quota's sentence, got {described:?}"
                );
            }
        }
    }

    assert!(
        rate_rows >= RATE_ROWS_EXPECTED && burst_rows >= 1,
        "case {RATE_SERVICE}: the rule must be asserted over the class — expected at least \
         {RATE_ROWS_EXPECTED} per-second rate rows and one burst row, got {rate_rows} and \
         {burst_rows}"
    );
}

/// RULE: every quota the listing publishes is reached by its own `QuotaCode` through
/// every read path, so a client that read a code out of the catalog resolves that
/// quota's record and no other.
#[tokio::test]
async fn every_listed_quota_reads_back_by_its_own_code() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    let listed = applied_listing(&caller, RATE_SERVICE).await;
    assert!(
        listed.len() > APPLIED_QUOTA_PAGE_SIZE,
        "case {RATE_SERVICE}: this service must publish more quotas than one unasked page holds, \
         got {}",
        listed.len()
    );

    for quota in &listed {
        let code = quota["QuotaCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case {RATE_SERVICE}: every listed quota carries a code"));
        let pair = json!({"ServiceCode": RATE_SERVICE, "QuotaCode": code});

        // The applied single read answers with the record the walked listing published
        // for that code; the AWS-default read answers with the same DEFINITION (its own
        // view publishes no applied level and an account-less ARN, which
        // `default_quota_catalog.rs` owns).
        let read = caller.ok("GetServiceQuota", pair.clone()).await["Quota"].clone();
        assert_eq!(
            read, *quota,
            "case {RATE_SERVICE}/{code} GetServiceQuota: expected the listed record, got {read}"
        );
        let default = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();
        for member in [
            "QuotaCode",
            "QuotaName",
            "Description",
            "Unit",
            "Value",
            "Adjustable",
            "GlobalQuota",
            "Period",
            "UsageMetric",
        ] {
            assert_eq!(
                default[member], quota[member],
                "case {RATE_SERVICE}/{code} GetAWSDefaultServiceQuota: expected {member} \
                 {:?}, got {:?}",
                quota[member], default[member]
            );
        }
        let filtered = caller.ok(OPERATION, pair.clone()).await["Quotas"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            filtered,
            vec![quota.clone()],
            "case {RATE_SERVICE}/{code}: the one-quota listing carries that quota alone"
        );
    }
}

/// RULE: the unasked applied page is bounded, a `NextToken` follows it whenever the
/// service has quotas past it, and chaining the token reports every quota of the
/// service exactly once.
#[tokio::test]
async fn an_unasked_page_is_bounded_and_its_token_chain_reports_each_quota_once() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    // Rows: services whose applied sets span the lengths the rule generalises over —
    // `iam` a handful (shorter than one unasked page), `acm` and `lambda` dozens, `ec2`
    // hundreds (past the largest page a caller may ask for).
    for service_code in ["iam", "acm", RATE_SERVICE, "ec2"] {
        let whole = applied_listing(&caller, service_code).await;
        let body = json!({"ServiceCode": service_code});

        let unasked = caller.ok(OPERATION, body.clone()).await;
        let page = unasked["Quotas"].as_array().cloned().unwrap_or_default();
        let served = APPLIED_QUOTA_PAGE_SIZE.min(whole.len());
        assert_eq!(
            page.len(),
            served,
            "case {service_code}: an unasked page holds the first {served} of {} quotas, got {}",
            whole.len(),
            page.len()
        );
        assert_eq!(
            !unasked["NextToken"].as_str().unwrap_or_default().is_empty(),
            served < whole.len(),
            "case {service_code}: a NextToken follows the unasked page exactly while quotas \
             remain ({served} of {} served), got {unasked}",
            whole.len()
        );

        // The chain walked with no page size of its own: every quota exactly once, in
        // the published order, and no code twice.
        let walked = walk_pages(&caller, OPERATION, &body, "Quotas", None).await;
        assert_eq!(
            walked, whole,
            "case {service_code}: the unasked token chain must report the published listing once"
        );
        let codes: BTreeSet<&str> = walked
            .iter()
            .filter_map(|quota| quota["QuotaCode"].as_str())
            .collect();
        assert_eq!(
            codes.len(),
            walked.len(),
            "case {service_code}: the walk reported {} records under {} distinct codes",
            walked.len(),
            codes.len()
        );
    }
}
