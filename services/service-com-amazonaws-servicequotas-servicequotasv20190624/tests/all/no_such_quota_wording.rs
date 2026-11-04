// SPDX-License-Identifier: CC0-1.0

//! How Service Quotas states that no quota answers to a `(ServiceCode, QuotaCode)`
//! pair, over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine. The harness (`emulator`, `client`,
//! `Client`) is `quota_workflow.rs`'s.
//!
//! The rule asserted, over a TABLE of pairs crossed with every operation that resolves
//! one: a request naming a `(ServiceCode, QuotaCode)` pair the catalog does not hold is
//! refused with the modelled `NoSuchResourceException`, in the service's ONE wording for
//! that failed lookup — the same FIXED sentence from whichever operation resolved the pair,
//! carrying nothing of the request, so neither code the caller sent appears in it. The
//! reads of one quota, its listing's `QuotaCode` filter, its change history and the write
//! that asks for an increase are all driven, so no handler gets to state the fact in a
//! sentence of its own. `auto_management_exclusions.rs` pins the same wording for the
//! Automatic Management exclusion writers, and `no_such_service_wording.rs` pins the
//! sentence a call SCOPED to a service is refused with instead.
//!
//! The pairs are read out of the catalog over the wire (`ListServices` +
//! `ListAWSDefaultServiceQuotas`) rather than spelled here, so the rows span services
//! and quota codes no issue cited.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::QUOTA_AND_SERVICE_ABSENT_MESSAGE;

use crate::quota_workflow::{client, default_quotas, emulator, service_inventory, Client};

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// The change history of one quota, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// A service the catalog publishes but holds no quotas for, so every pair naming it is
/// a pair the catalog does not hold (the same service `unasked_page_size.rs` lists an
/// empty quota set for).
const SERVICE_WITHOUT_QUOTAS: &str = "kinesisanalytics";

/// The requests that resolve a `(ServiceCode, QuotaCode)` pair, as
/// `(what the call is, operation, body)` — every operation of this service that looks
/// one up, including the listing's `QuotaCode` filter on each plane the listing
/// reports.
fn pair_requests(service_code: &str, quota_code: &str) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "the single applied read",
            "GetServiceQuota",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the single default read",
            "GetAWSDefaultServiceQuota",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the applied listing's QuotaCode filter",
            "ListServiceQuotas",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the applied listing's QuotaCode filter on the resource plane",
            "ListServiceQuotas",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "QuotaAppliedAtLevel": "RESOURCE",
            }),
        ),
        (
            "the applied listing's QuotaCode filter on both planes, paged",
            "ListServiceQuotas",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "QuotaAppliedAtLevel": "ALL",
                "MaxResults": 1,
            }),
        ),
        (
            "the change history of one quota",
            HISTORY,
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the write that asks for an increase",
            "RequestServiceQuotaIncrease",
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "DesiredValue": 10.0,
            }),
        ),
    ]
}

/// The `Message` of the refusal `operation` answers `body` with, after asserting it IS
/// the modelled `NoSuchResourceException` on this front end's client-fault status.
///
/// `pub(crate)` so the suite that pins the OTHER half of the same rule — the sentence a
/// `ServiceCode` the catalog does not publish is stated in — asserts the refusal through
/// this one reader instead of a copy of it.
pub(crate) async fn no_such_resource_message(
    client: &Client,
    case: &str,
    operation: &str,
    body: &Value,
) -> String {
    let (status, parsed) = client.call(operation, body).await;
    let member = |name: &str| parsed.get(name).and_then(Value::as_str).map(str::to_string);
    let code = member("__type").unwrap_or_else(|| "<no __type>".to_string());
    assert_eq!(
        status, CLIENT_FAULT,
        "case {case}: expected {operation} to answer HTTP {CLIENT_FAULT}, got {status} \
         with {parsed}"
    );
    assert_eq!(
        code, "NoSuchResourceException",
        "case {case}: expected NoSuchResourceException, got {code:?} in {parsed}"
    );
    member("Message")
        .unwrap_or_else(|| panic!("case {case}: expected a Message member, got {parsed}"))
}

/// The first `wanted` service codes the catalog publishes a quota for, each with the
/// first `QuotaCode` it holds — walked over the wire through the harness's own catalog
/// readers (`quota_workflow::service_inventory` / `default_quotas`), so this suite
/// carries neither a copy of the catalog nor a second walk of it.
pub(crate) async fn catalog_services(client: &Client, wanted: usize) -> Vec<(String, String)> {
    let mut picked: Vec<(String, String)> = Vec::new();
    for service in service_inventory(client).await {
        if picked.len() == wanted {
            break;
        }
        let Some(service_code) = service.get("ServiceCode").and_then(Value::as_str) else {
            continue;
        };
        let first = default_quotas(client, service_code).await;
        let first = first
            .first()
            .and_then(|quota| quota.get("QuotaCode"))
            .and_then(Value::as_str);
        if let Some(quota_code) = first {
            picked.push((service_code.to_string(), quota_code.to_string()));
        }
    }
    picked
}

/// The pairs the catalog does not hold, as `(case, ServiceCode, QuotaCode)`.
///
/// The rows differ along WHAT the pair gets wrong and in the FORM of the code: a code
/// no service holds, a code of ANOTHER service the catalog does hold, a real code in
/// the wrong case, a code at the longest length the member allows (`@length 1..128` on
/// `QuotaCode`), the shortest code the member allows, and a pair under a service the
/// catalog publishes no quotas for at all. The services and the real codes come from
/// the catalog read over the wire.
async fn unheld_pairs(client: &Client) -> Vec<(String, String, String)> {
    let catalog = catalog_services(client, 2).await;
    assert!(
        catalog.len() == 2,
        "case setup: expected two catalog services each publishing a quota, got {catalog:?}"
    );
    let (first_service, first_code) = catalog[0].clone();
    let (second_service, second_code) = catalog[1].clone();

    let rows = vec![
        (
            "a quota code no service holds".to_string(),
            first_service.clone(),
            "L-00000000".to_string(),
        ),
        (
            format!("the real quota code {second_code} of another service"),
            first_service.clone(),
            second_code,
        ),
        (
            format!("the real quota code {first_code} in the wrong case"),
            first_service.clone(),
            first_code.to_lowercase(),
        ),
        (
            // `@length 1..128` on `QuotaCode`: the longest code the member allows.
            "a quota code at the length maximum of the member".to_string(),
            second_service.clone(),
            format!("L{}", "a".repeat(127)),
        ),
        (
            // And its other bound: the shortest code the member's
            // `@pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$` admits, two characters.
            "the shortest quota code the member admits".to_string(),
            second_service,
            "La".to_string(),
        ),
        (
            "a service the catalog publishes no quotas for".to_string(),
            SERVICE_WITHOUT_QUOTAS.to_string(),
            "L-12345678".to_string(),
        ),
    ];
    // A row only tests the rule while its pair really is one the catalog does not hold:
    // a quota code CAN coincide across services, and a code with no letter case has no
    // case to get wrong. Each row is checked against the catalog over the wire and
    // dropped if the pair turns out to be held, so the table follows whatever the
    // catalog publishes instead of today's first two services.
    let mut unheld = Vec::new();
    for (why, service_code, quota_code) in rows {
        let (status, _) = client
            .call(
                "GetAWSDefaultServiceQuota",
                &json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await;
        if status != 200 {
            unheld.push((why, service_code, quota_code));
        }
    }
    assert!(
        unheld.len() >= 4,
        "case setup: expected the table to span at least four pairs the catalog does not \
         hold, got {unheld:?}"
    );
    unheld
}

/// The sentence this service states the fact in. Spelled here so the suite pins the
/// WORDING as well as its single sourcing: comparing a refusal against
/// `QUOTA_AND_SERVICE_ABSENT_MESSAGE` alone would stay green if that constant's own
/// sentence drifted.
const WORDING: &str = "The request failed because the specified quota and service do not \
                      exist.";

/// A `QuotaCode` a published service holds no quota under is refused with ONE wording,
/// whichever operation resolved it — the service's own fixed sentence for the fact, with
/// nothing of the request in it.
#[tokio::test]
async fn every_operation_states_an_unheld_pair_in_one_wording() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Every row must produce the same sentence, so a second wording for the fact cannot
    // hide behind differing codes.
    let mut stated: Vec<(String, String)> = Vec::new();

    for (why, service_code, quota_code) in unheld_pairs(&client).await {
        for (what, operation, body) in pair_requests(&service_code, &quota_code) {
            let case = format!("{why}, through {what} ({operation})");
            let message = no_such_resource_message(&client, &case, operation, &body).await;
            assert_eq!(
                message, QUOTA_AND_SERVICE_ABSENT_MESSAGE,
                "case {case}: expected the refusal to state \
                 {QUOTA_AND_SERVICE_ABSENT_MESSAGE:?}, got {message:?}"
            );
            // The sentence is FIXED: a row differing only in the codes it names gets the
            // same string back, which the comparison across rows below pins.
            stated.push((case, message));
        }
    }

    let (first_case, first) = stated
        .first()
        .cloned()
        .expect("the table drives at least one call");
    assert!(
        !first.is_empty(),
        "case {first_case}: expected a non-empty sentence, got {first:?}"
    );
    for (case, message) in &stated {
        assert_eq!(
            message, &first,
            "case {case}: expected the one wording {first:?}, got {message:?}"
        );
        // And the wording itself, not only that every operation shares one.
        assert_eq!(
            message, WORDING,
            "case {case}: expected the sentence {WORDING:?}, got {message:?}"
        );
    }
}

/// The other half of the rule: a pair the catalog DOES hold is served, so the refusal
/// above reports a missing quota rather than a filter that never matches.
#[tokio::test]
async fn a_pair_the_catalog_holds_is_served_by_every_operation_that_resolves_one() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    for (service_code, quota_code) in catalog_services(&client, 3).await {
        for (what, operation, body) in [
            (
                "the single applied read",
                "GetServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            ),
            (
                "the applied listing's QuotaCode filter",
                "ListServiceQuotas",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            ),
            (
                "the change history of one quota",
                HISTORY,
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            ),
        ] {
            let case = format!("{service_code}/{quota_code}, through {what} ({operation})");
            let (status, parsed) = client.call(operation, &body).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
        }

        // And the listing's filter really is a filter: it reports the named quota only.
        let listed = client
            .ok(
                "ListServiceQuotas",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await;
        let codes: Vec<String> = listed
            .get("Quotas")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|quota| quota.get("QuotaCode").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert!(
            !codes.is_empty() && codes.iter().all(|code| code == &quota_code),
            "case {service_code}/{quota_code}, the filtered listing: expected only that \
             QuotaCode, got {codes:?}"
        );
    }
}
