// SPDX-License-Identifier: CC0-1.0

//! How Service Quotas states that a request names a `ServiceCode` the catalog does not
//! publish, over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine. The harness (`emulator`, `client`,
//! `Client`) is `quota_workflow.rs`'s, the refusal reader is `no_such_quota_wording.rs`'s
//! and the template plane's two-service harness is `quota_request_template.rs`'s.
//!
//! The rule asserted, over a TABLE of unpublished service codes crossed with every
//! operation that resolves one: the refusal is the modelled `NoSuchResourceException`
//! stating the service's own FIXED sentence for the fact, so nothing the request carried
//! is interpolated into it — and WHICH fact it states follows from what the call named:
//!
//! * a call SCOPED to the service — a listing keyed by a `ServiceCode`, including the
//!   change history of one quota — states that the service is not available at the
//!   endpoint the call was made at, which is what a caller can act on;
//! * a call that resolves ONE quota by its `(ServiceCode, QuotaCode)` pair states that the
//!   quota and the service do not exist, one sentence for the failed pair lookup whichever
//!   half of it is the missing one.
//!
//! Its other half is pinned here too: a service the catalog DOES publish, named with a code
//! it holds no quota under, is the same failed pair lookup (`no_such_quota_wording.rs`
//! spans that across the same operations).
//!
//! The published codes the rows are built from are read out of the catalog over the wire
//! (`ListServices` + `ListAWSDefaultServiceQuotas`), so the table follows whatever the
//! catalog publishes rather than carrying a copy of it.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::service::refusals::{
    QUOTA_AND_SERVICE_ABSENT_MESSAGE, SERVICE_NOT_AVAILABLE_MESSAGE,
};

use crate::no_such_quota_wording::{catalog_services, no_such_resource_message};
use crate::quota_request_template::Emulator;
use crate::quota_workflow::{client, emulator, service_inventory, Client};

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// The service-wide change history, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The change history of ONE quota — a listing keyed by a `ServiceCode`, which happens
/// to name a `QuotaCode` under it as well.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// A `QuotaCode` no service of the catalog holds, well within the member's
/// `@length 1..128`.
const UNHELD_QUOTA_CODE: &str = "L-00000000";

/// The sentence a call scoped to a service the endpoint does not serve is refused with,
/// down to the trailing space the service sends it with. Spelled here so the suite pins
/// the WORDING as well as its single sourcing: comparing a refusal against the crate's
/// constant alone would stay green if that constant's own sentence drifted.
const SERVICE_WORDING: &str =
    "This service is not available in the current Region. Choose a different Region or a \
     different service. ";

/// The sentence a call that resolves ONE quota by its pair is refused with when the pair
/// resolves to no quota — whichever half of it is the half that is missing — spelled here
/// for the same reason.
const PAIR_WORDING: &str = "The request failed because the specified quota and service do not \
                            exist.";

/// The LISTINGS that resolve a `ServiceCode` against the catalog, as
/// `(what the call is, operation, body)`: the four of them that take one, each also in a
/// filtered and paged form, so the lookup is seen to be answered before any filter of the
/// listing is.
fn listing_requests(
    service_code: &str,
    quota_code: &str,
) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "the applied listing",
            "ListServiceQuotas",
            json!({"ServiceCode": service_code}),
        ),
        (
            "the applied listing's QuotaCode filter",
            "ListServiceQuotas",
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the default listing",
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": service_code, "MaxResults": 1}),
        ),
        (
            "the service-wide change history",
            HISTORY,
            json!({"ServiceCode": service_code}),
        ),
        (
            "the service-wide change history, filtered and paged",
            HISTORY,
            json!({"ServiceCode": service_code, "Status": "PENDING", "MaxResults": 1}),
        ),
        (
            "the change history of one quota",
            HISTORY_BY_QUOTA,
            json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
        ),
        (
            "the change history of one quota, filtered and paged",
            HISTORY_BY_QUOTA,
            json!({
                "ServiceCode": service_code,
                "QuotaCode": quota_code,
                "Status": "PENDING",
                "QuotaRequestedAtLevel": "ALL",
                "MaxResults": 1,
            }),
        ),
    ]
}

/// The calls that resolve ONE quota by its `(ServiceCode, QuotaCode)` pair: the two reads
/// of one quota and the write that asks for an increase on it.
fn single_quota_requests(
    service_code: &str,
    quota_code: &str,
) -> Vec<(&'static str, &'static str, Value)> {
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

/// Every call that resolves a `ServiceCode` at all, both classes together: what the
/// suites that assert something common to all of them drive.
fn service_code_requests(
    service_code: &str,
    quota_code: &str,
) -> Vec<(&'static str, &'static str, Value)> {
    let mut requests = listing_requests(service_code, quota_code);
    requests.extend(single_quota_requests(service_code, quota_code));
    requests
}

/// Every `ServiceCode` `ListServices` publishes, walked over the wire.
async fn published_service_codes(client: &Client) -> Vec<String> {
    service_inventory(client)
        .await
        .iter()
        .filter_map(|service| service.get("ServiceCode").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The service codes the catalog does not publish, as `(case, ServiceCode)`.
///
/// The rows differ in the FORM of the code and in HOW it misses the catalog: a
/// well-formed code naming nothing, a published code mis-cased, one with a suffix, one
/// with its last character dropped, a hyphen-separated phrase, and the two codes at the
/// bounds of what the member admits — 63 characters and the two-character minimum of
/// `@length 1..63` with `@pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$`
/// (`smithy_query(query="shape", shape="ServiceCode")`). The published codes the mis-cased
/// and suffixed rows are built from come from the catalog read over the wire.
///
/// A row only tests the rule while its code really is one the catalog does not publish —
/// dropping a character off one published code can land on another — so every row is
/// checked against the inventory and dropped if the catalog turns out to publish it.
async fn unpublished_service_codes(client: &Client) -> Vec<(String, String)> {
    let published = published_service_codes(client).await;
    assert!(
        published.len() >= 2,
        "case setup: expected the catalog to publish at least two services, got {published:?}"
    );
    let first = published[0].clone();
    let second = published[1].clone();
    let shortened: String = second.chars().take(second.chars().count() - 1).collect();

    let rows = vec![
        (
            "a well-formed code naming no service".to_string(),
            "unpublishedservice".to_string(),
        ),
        (
            format!("the published code {first} in upper case"),
            first.to_uppercase(),
        ),
        (
            format!("the published code {first} with a suffix"),
            format!("{first}-not"),
        ),
        (
            format!("the published code {second} with its last character dropped"),
            shortened,
        ),
        (
            "a hyphen-separated phrase naming no service".to_string(),
            "no-such-aws-service".to_string(),
        ),
        (
            // The longest code the member admits.
            "a code at the length maximum of the member".to_string(),
            format!("Z{}", "a".repeat(62)),
        ),
        (
            // And its other bound: the shortest the member's pattern admits.
            "the shortest code the member admits".to_string(),
            "Zz".to_string(),
        ),
    ];

    let unpublished: Vec<(String, String)> = rows
        .into_iter()
        .filter(|(_, code)| !published.contains(code))
        .collect();
    assert!(
        unpublished.len() >= 5,
        "case setup: expected the table to span at least five codes the catalog does not \
         publish, got {unpublished:?}"
    );
    unpublished
}

/// A `ServiceCode` the catalog does not publish is refused with ONE wording per class of
/// call, whichever operation resolved it and whatever the row's codes are — the service's
/// own fixed sentence for the fact, naming neither the code the caller sent nor the quota
/// code beside it.
#[tokio::test]
async fn every_operation_states_an_unpublished_service_code_in_one_wording() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // A quota code the catalog really does publish, for the rows that carry one: the
    // refusal must still be about the service, not about a code that exists elsewhere.
    let catalog = catalog_services(&client, 1).await;
    let (_, published_quota_code) = catalog
        .first()
        .cloned()
        .expect("case setup: the catalog publishes at least one quota");

    // Every row of each class must produce the same sentence, so a second wording for
    // one fact cannot hide behind differing codes.
    let mut calls = 0usize;

    for (why, service_code) in unpublished_service_codes(&client).await {
        for quota_code in [published_quota_code.as_str(), UNHELD_QUOTA_CODE] {
            for (class, expected, wording, requests) in [
                (
                    "scoped to the service",
                    SERVICE_NOT_AVAILABLE_MESSAGE,
                    SERVICE_WORDING,
                    listing_requests(&service_code, quota_code),
                ),
                (
                    "resolving one quota",
                    QUOTA_AND_SERVICE_ABSENT_MESSAGE,
                    PAIR_WORDING,
                    single_quota_requests(&service_code, quota_code),
                ),
            ] {
                for (what, operation, body) in requests {
                    let case =
                        format!("{why}, QuotaCode {quota_code}, through {what} ({operation})");
                    let message = no_such_resource_message(&client, &case, operation, &body).await;
                    // The crate's one sentence for the fact, and the sentence itself:
                    // comparing against the constant alone would stay green if its own
                    // wording drifted.
                    assert_eq!(
                        message, expected,
                        "case {case}: expected a call {class} to state {expected:?}, got \
                         {message:?}"
                    );
                    assert_eq!(
                        message, wording,
                        "case {case}: expected the sentence {wording:?} for a call {class}, got \
                         {message:?}"
                    );
                    // The sentence carries nothing of the request: not the service code
                    // that named no service, and not the quota code beside it.
                    assert!(
                        !message.contains(&service_code) && !message.contains(quota_code),
                        "case {case}: expected a refusal naming neither ServiceCode \
                         {service_code:?} nor QuotaCode {quota_code:?}, got {message:?}"
                    );
                    calls += 1;
                }
            }
        }
    }

    assert!(
        calls >= 50,
        "case setup: expected the table to drive every operation over at least five \
         unpublished codes, got {calls} calls"
    );
}

/// The other half of the rule: a service the catalog DOES publish, named with a code it
/// holds no quota under, is a failed PAIR lookup — stated in the pair sentence rather than
/// in the one a call scoped to a service gets.
#[tokio::test]
async fn a_published_service_with_an_unheld_quota_code_states_the_pair_fact() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    for (service_code, held_quota_code) in catalog_services(&client, 3).await {
        for (what, operation, body) in service_code_requests(&service_code, UNHELD_QUOTA_CODE) {
            // The listings that take no `QuotaCode` are served for a published service,
            // so only the rows that resolve the code are refusals here.
            if !body
                .get("QuotaCode")
                .and_then(Value::as_str)
                .is_some_and(|code| code == UNHELD_QUOTA_CODE)
            {
                continue;
            }
            let case = format!("{service_code}/{UNHELD_QUOTA_CODE}, through {what} ({operation})");
            let message = no_such_resource_message(&client, &case, operation, &body).await;
            assert_eq!(
                message, PAIR_WORDING,
                "case {case}: expected the refusal to state {PAIR_WORDING:?}, got {message:?}"
            );
            assert_eq!(
                message, QUOTA_AND_SERVICE_ABSENT_MESSAGE,
                "case {case}: expected the crate's one sentence for a failed pair lookup, got \
                 {message:?}"
            );
            assert_ne!(
                message, SERVICE_NOT_AVAILABLE_MESSAGE,
                "case {case}: expected the pair sentence for a published service, got the \
                 service one"
            );
        }

        // And the pair the catalog does hold is served, so the refusals above report a
        // missing quota rather than a lookup that fails for everything.
        let case = format!("{service_code}/{held_quota_code}, through {HISTORY_BY_QUOTA}");
        let (status, parsed) = client
            .call(
                HISTORY_BY_QUOTA,
                &json!({"ServiceCode": service_code, "QuotaCode": held_quota_code}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {parsed}"
        );
    }
}

/// The bound the rule stops at: a value OUTSIDE the modelled `ServiceCode` constraints
/// names nothing the catalog could publish, so it is bad INPUT
/// (`IllegalArgumentException`) rather than a missing resource, and never earns the
/// service sentence.
#[tokio::test]
async fn a_service_code_outside_the_member_is_bad_input_rather_than_a_missing_service() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // `@length 1..63` with `@pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$`
    // (`smithy_query(query="shape", shape="ServiceCode")`): one character is below what
    // the pattern admits, 64 characters is one past the length maximum, and an
    // underscore is outside the pattern's alphabet.
    let past_the_maximum = format!("Z{}", "a".repeat(63));
    let rows = [
        ("one character, below the pattern's minimum", "z"),
        ("64 characters, one past the length maximum", &past_the_maximum[..]),
        ("an underscore, outside the pattern", "no_such_service"),
    ];

    for (why, service_code) in rows {
        for (what, operation, body) in service_code_requests(service_code, UNHELD_QUOTA_CODE) {
            let case = format!("{why}, through {what} ({operation})");
            let (status, code) = client.refused(operation, body).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_FAULT, "IllegalArgumentException"),
                "case {case}: expected HTTP {CLIENT_FAULT} IllegalArgumentException, got \
                 {status} {code}"
            );
        }
    }
}

/// The template write resolves the same pair, and states the same two facts: its plane
/// sits behind an organization gate, so it is driven through the two-service harness.
#[tokio::test]
async fn the_template_write_states_the_same_two_facts() {
    let emulator = Emulator::in_organization("ALL").await;
    let operation = "PutServiceQuotaIncreaseRequestIntoTemplate";
    // The pair every other row of this harness's suite uses for a published quota
    // (`vpc` `L-F678F1CE`), so the service rows below differ from it only in the half of
    // the pair under test.
    let published_service = "vpc";
    let published_quota = "L-F678F1CE";

    for (why, service_code) in [
        ("a well-formed code naming no service", "unpublishedservice"),
        (
            "the published code vpc in upper case",
            &published_service.to_uppercase()[..],
        ),
        ("the published code vpc with a suffix", "vpc-not"),
    ] {
        let case = format!("{why}, through the template write");
        let (status, code, message) = emulator
            .refused_at_home(
                operation,
                json!({
                    "ServiceCode": service_code,
                    "QuotaCode": published_quota,
                    "AwsRegion": "us-east-1",
                    "DesiredValue": 100.0,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_FAULT, "NoSuchResourceException"),
            "case {case}: expected HTTP {CLIENT_FAULT} NoSuchResourceException, got {status} \
             {code} with {message:?}"
        );
        // The write resolves ONE quota by its pair, so it states the pair fact.
        assert_eq!(
            message, PAIR_WORDING,
            "case {case}: expected the refusal to state {PAIR_WORDING:?}, got {message:?}"
        );
    }

    // And the quota half, through the same write.
    let case = format!("{published_service}/{UNHELD_QUOTA_CODE}, through the template write");
    let (status, code, message) = emulator
        .refused_at_home(
            operation,
            json!({
                "ServiceCode": published_service,
                "QuotaCode": UNHELD_QUOTA_CODE,
                "AwsRegion": "us-east-1",
                "DesiredValue": 100.0,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (CLIENT_FAULT, "NoSuchResourceException", PAIR_WORDING),
        "case {case}: expected HTTP {CLIENT_FAULT} NoSuchResourceException stating \
         {PAIR_WORDING:?}, got {status} {code} with {message:?}"
    );
}
