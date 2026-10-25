// SPDX-License-Identifier: CC0-1.0

//! The `ServiceCode` this service's quota refusal carries.
//!
//! The rule under test: a member whose own model documentation states the single value it
//! ALWAYS carries is emitted with that documented value, whatever the request — it is not
//! derived from the service's wire identity (its endpoint prefix / SigV4 signing name).
//! For this service the model states it of `ServiceQuotaExceededException$ServiceCode`:
//! "The service code that exceeded quota. It will always be
//! “AWSBillingAndCostManagementDataExports”" (smithy_query query="doc"
//! shape="ServiceQuotaExceededException" member="ServiceCode"); the same sentence
//! documents `ThrottlingException$ServiceCode`, the only other member of this service's
//! model that fixes its value in prose.
//!
//! `CreateExport` past the store's bound is the one request of this service that reaches
//! such a member, so the rule is asserted over a TABLE of requests that all reach it and
//! differ along everything it generalises over: every shape of export definition the
//! crate's own case table carries (absent vs present description, absent vs present table
//! configurations and bucket owner, each published value of every `S3OutputConfigurations`
//! enum, tag counts from zero to three), export names at both published bounds of
//! `Export.Name`'s `@length 1..128`, and the region the request is signed for. The value
//! must not move for any of them, and the `QuotaCode` sibling — which the model leaves to
//! the service ("The quota code that was exceeded") — must still be reported.

use std::collections::BTreeSet;

use serde_json::Value;

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::MAX_EXPORTS;

use crate::export_lifecycle::{cases, error_code, Client};

/// The value the model fixes for `ServiceCode`, written out here rather than taken from
/// the crate so the test pins the documented sentence itself (smithy_query query="doc"
/// shape="ServiceQuotaExceededException" member="ServiceCode").
const DOCUMENTED_SERVICE_CODE: &str = "AWSBillingAndCostManagementDataExports";

/// This service's endpoint prefix, which is also its SigV4 signing name and its ARN
/// namespace — the identity the fixed member must NOT be derived from.
const ENDPOINT_PREFIX: &str = "bcm-data-exports";

/// `ServiceCode` targets `GenericString`: `@length 0..1024`, pattern `^[\S\s]*$`
/// (smithy_query shape `ServiceQuotaExceededException`, member `ServiceCode`).
const SERVICE_CODE_MAX_LEN: usize = 1024;

/// `Export.Name`: `@length 1..128` (smithy_query shape `Export`, member `Name`).
const NAME_MAX_LEN: usize = 128;

/// The HTTP status and error code of the modelled quota refusal
/// (`ServiceQuotaExceededException`, `@httpError 402`).
const QUOTA_STATUS: u16 = 402;

/// The regions the table signs its refused requests for, so the fixed member is also
/// checked against a request whose credential scope names a region other than the
/// default one.
const SIGNING_REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// One row: the name a refused `CreateExport` carries and the region it is signed for,
/// paired with the definition shape it submits.
struct Row {
    label: String,
    name: String,
    region: &'static str,
    request: Value,
}

/// Every refused request of the table: each definition shape the crate's case table
/// carries, plus a name at each published bound of `Export.Name`'s length constraint,
/// cycled over the signing regions.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, case) in cases().into_iter().enumerate() {
        let mut case = case;
        // A FRESH name, so the request reaches the store's bound rather than the
        // duplicate-name refusal that is judged before it.
        let name = format!("past_the_bound_{index:04}");
        case.set_name(&name);
        rows.push(Row {
            label: format!(
                "definition row {index} ({} tags, tables {:?})",
                case.tag_count(),
                case.queried_tables()
            ),
            name,
            region: SIGNING_REGIONS[index % SIGNING_REGIONS.len()],
            request: case.create_request(),
        });
    }
    // Two more rows for the published bounds of `Export.Name`'s length constraint: a
    // one-character name (the shortest the constraint allows) and a 128-character one
    // (the longest). The rows above all carry names of an ordinary middle length.
    for (label, name) in [
        ("name at the minimum length".to_string(), "q".to_string()),
        (
            "name at the maximum length".to_string(),
            "Q".repeat(NAME_MAX_LEN),
        ),
    ] {
        let mut case = cases().remove(0);
        case.set_name(&name);
        rows.push(Row {
            label,
            name,
            region: SIGNING_REGIONS[0],
            request: case.create_request(),
        });
    }
    rows
}

/// Fill the store to its bound, then every refused request reports the ServiceCode the
/// model fixes — and reports the same one for all of them.
#[tokio::test]
async fn the_quota_refusal_reports_the_service_code_the_model_fixes() {
    let client = Client::new();
    let template = cases().remove(0);
    for index in 0..MAX_EXPORTS {
        let mut filler = template.clone();
        filler.set_name(&format!("bounded_{index:04}"));
        client.ok("CreateExport", &filler.create_request()).await;
    }

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for row in rows() {
        let label = &row.label;
        let region = row.region;
        let (status, body) = client
            .call_in_region(region, "CreateExport", &row.request)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (QUOTA_STATUS, "ServiceQuotaExceededException"),
            "case {label:?} in {region}: creating past the bound must be the quota \
             refusal, got {status} with {body}"
        );
        let service_code = body["ServiceCode"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?} in {region}: the refusal must carry the required \
                     ServiceCode member, got {body}"
                )
            })
            .to_string();
        assert_eq!(
            service_code, DOCUMENTED_SERVICE_CODE,
            "case {label:?} in {region}: expected the fixed ServiceCode \
             {DOCUMENTED_SERVICE_CODE:?}, got {service_code:?}"
        );
        assert_ne!(
            service_code, ENDPOINT_PREFIX,
            "case {label:?} in {region}: the fixed ServiceCode must not be this service's \
             endpoint prefix, got {service_code:?}"
        );
        assert!(
            service_code.len() <= SERVICE_CODE_MAX_LEN,
            "case {label:?} in {region}: ServiceCode must satisfy the modelled length \
             0..{SERVICE_CODE_MAX_LEN}, got {} characters in {service_code:?}",
            service_code.len()
        );
        // A value the model fixes restates nothing of the request.
        assert!(
            !service_code.contains(&row.name) && !service_code.contains(region),
            "case {label:?} in {region}: a fixed member must not restate request content, \
             got {service_code:?}"
        );
        // The siblings the model does leave to the service are still reported: the quota
        // the refusal enforces, and its required message.
        assert!(
            body["QuotaCode"].as_str().is_some_and(|code| !code.is_empty()),
            "case {label:?} in {region}: the refusal must name the quota it enforces, got \
             {body}"
        );
        assert!(
            body["Message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {label:?} in {region}: the refusal must carry a Message, got {body}"
        );
        reported.insert(service_code);
    }

    assert_eq!(
        reported,
        BTreeSet::from([DOCUMENTED_SERVICE_CODE.to_string()]),
        "every refused request must report the SAME fixed ServiceCode, got {reported:?}"
    );
}
