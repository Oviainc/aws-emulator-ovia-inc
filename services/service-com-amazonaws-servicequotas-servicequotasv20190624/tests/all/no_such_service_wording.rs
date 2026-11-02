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
//! operation that resolves one: a lookup fails on the OUTERMOST thing the request named
//! that does not exist, so a `ServiceCode` the catalog does not publish is refused with
//! the modelled `NoSuchResourceException` stating the absent SERVICE — in the service's
//! one wording for that fact, with the `ServiceCode` of the request filled in — whether
//! the code arrives alone or as half of a `(ServiceCode, QuotaCode)` pair, and never
//! attributing the failure to the quota code the caller supplied. Its other half is
//! pinned here too: a service the catalog DOES publish, named with a code it holds no
//! quota under, still states the absent QUOTA (`no_such_quota_wording.rs` spans that
//! fact across the same operations).
//!
//! The published codes the rows are built from are read out of the catalog over the wire
//! (`ListServices` + `ListAWSDefaultServiceQuotas`), so the table follows whatever the
//! catalog publishes rather than carrying a copy of it.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    no_such_quota_message, no_such_service_message,
};

use crate::no_such_quota_wording::{catalog_services, no_such_resource_message};
use crate::quota_request_template::Emulator;
use crate::quota_workflow::{client, emulator, service_inventory, Client};

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// The service-wide change history, whose name is too long to repeat in every row.
const HISTORY: &str = "ListRequestedServiceQuotaChangeHistory";

/// The change history of ONE quota — the listing the `ServiceCode` fact used to be
/// stated in the quota sentence by.
const HISTORY_BY_QUOTA: &str = "ListRequestedServiceQuotaChangeHistoryByQuota";

/// A `QuotaCode` no service of the catalog holds, well within the member's
/// `@length 1..128`.
const UNHELD_QUOTA_CODE: &str = "L-00000000";

/// The sentence this service states the fact in, with the `ServiceCode` of the request
/// blanked out. Spelled here so the suite pins the WORDING as well as its single
/// sourcing: comparing a refusal against `no_such_service_message` alone would stay
/// green if the helper's own sentence drifted.
const WORDING: &str = "no service quotas are available for ServiceCode '<ServiceCode>'";

/// The requests that resolve a `ServiceCode` against the catalog, as
/// `(what the call is, operation, body)`: the four listings that take one, and the two
/// reads and the write that take it as half of a `(ServiceCode, QuotaCode)` pair. The
/// filtered and paged rows are there so the lookup is seen to be answered before any
/// filter of the listing is.
fn service_code_requests(
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

/// A `ServiceCode` the catalog does not publish is refused with ONE wording, whichever
/// operation resolved it, and the sentence names the absent service rather than the
/// `QuotaCode` the caller supplied.
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

    // The sentence with the request's code blanked out: every row must produce the same
    // one, so a second wording for the fact cannot hide behind differing codes.
    let mut templates: Vec<(String, String)> = Vec::new();

    for (why, service_code) in unpublished_service_codes(&client).await {
        let expected = no_such_service_message(&service_code);
        for quota_code in [published_quota_code.as_str(), UNHELD_QUOTA_CODE] {
            for (what, operation, body) in service_code_requests(&service_code, quota_code) {
                let case = format!("{why}, QuotaCode {quota_code}, through {what} ({operation})");
                let message = no_such_resource_message(&client, &case, operation, &body).await;
                assert_eq!(
                    message, expected,
                    "case {case}: expected the refusal to state {expected:?}, got {message:?}"
                );
                // The fact is the absent SERVICE: the quota sentence would name the
                // code the caller supplied, and this one must not.
                assert!(
                    !message.contains(quota_code),
                    "case {case}: expected the refusal to attribute the failure to the \
                     absent service, not to QuotaCode {quota_code:?}, got {message:?}"
                );
                assert_ne!(
                    message,
                    no_such_quota_message(&service_code, quota_code),
                    "case {case}: expected the service sentence, got the quota one"
                );
                templates.push((case, message.replace(&service_code, "<ServiceCode>")));
            }
        }
    }

    let (first_case, first_template) = templates
        .first()
        .cloned()
        .expect("the table drives at least one call");
    assert!(
        first_template.contains("<ServiceCode>"),
        "case {first_case}: expected one sentence carrying the ServiceCode, got \
         {first_template:?}"
    );
    for (case, template) in &templates {
        assert_eq!(
            template, &first_template,
            "case {case}: expected the one wording {first_template:?}, got {template:?}"
        );
        // And the wording itself, not only that every operation shares one.
        assert_eq!(
            template, WORDING,
            "case {case}: expected the sentence {WORDING:?}, got {template:?}"
        );
    }
}

/// The other half of the rule: a service the catalog DOES publish, named with a code it
/// holds no quota under, keeps stating the absent QUOTA — so the two facts stay two
/// sentences instead of collapsing into one.
#[tokio::test]
async fn a_published_service_with_an_unheld_quota_code_states_the_quota_fact() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    for (service_code, held_quota_code) in catalog_services(&client, 3).await {
        let expected = no_such_quota_message(&service_code, UNHELD_QUOTA_CODE);
        for (what, operation, body) in service_code_requests(&service_code, UNHELD_QUOTA_CODE) {
            // The listings that take no `QuotaCode` are served for a published service,
            // so only the rows that resolve the pair are refusals here.
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
                message, expected,
                "case {case}: expected the refusal to state {expected:?}, got {message:?}"
            );
            assert_ne!(
                message,
                no_such_service_message(&service_code),
                "case {case}: expected the quota sentence for a published service, got the \
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
        let expected = no_such_service_message(service_code);
        assert_eq!(
            message, expected,
            "case {case}: expected the refusal to state {expected:?}, got {message:?}"
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
    let expected = no_such_quota_message(published_service, UNHELD_QUOTA_CODE);
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (CLIENT_FAULT, "NoSuchResourceException", expected.as_str()),
        "case {case}: expected HTTP {CLIENT_FAULT} NoSuchResourceException stating \
         {expected:?}, got {status} {code} with {message:?}"
    );
}
