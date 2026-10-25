// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the create/read path of a custom domain name AT the account's
//! quota: `CreateDomainName` and `GetDomainName`.
//!
//! The RULE under test: an account holds at most [`MAX_DOMAIN_NAMES`] custom domain names
//! — API Gateway's published quota — so the create AT the bound is stored and readable,
//! every create ONE PAST it earns the modelled `LimitExceededException` whatever shape
//! the request takes, and a refused create leaves the store exactly as it was (the record
//! at the bound still reads, the refused name resolves to nothing).
//!
//! The sibling `domain_name_collection` suite asserts WHAT a created domain name reports
//! and how it is addressed; `domain_name_create_bounds` asserts the tag rule a create
//! applies. [`Gateway`], [`Answer`] and [`assert_failure`] are this crate's one wire-test
//! harness, reused rather than standing a second emulator up another way.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::MAX_DOMAIN_NAMES;

use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};

/// `CreateDomainName` with `body`.
async fn create_domain(gateway: &Gateway, body: Value) -> Answer {
    gateway.call("POST", "/domainnames", Some(body)).await
}

/// `GetDomainName` for `domain_name`.
async fn get_domain(gateway: &Gateway, domain_name: &str) -> Answer {
    gateway
        .call("GET", &format!("/domainnames/{domain_name}"), None)
        .await
}

/// The nth distinct domain name this suite creates — a name the service's domain-name
/// grammar accepts, so only the rule under test can refuse it.
fn nth_name(n: usize) -> String {
    format!("bound-{n}.example.com")
}

#[tokio::test]
async fn an_account_holds_at_most_its_quota_of_custom_domain_names() {
    let gateway = Gateway::new();

    // Fill the account to the published quota. Every create is accepted: the bound is on
    // the number of records, so the LAST one that fits is still stored.
    for index in 0..MAX_DOMAIN_NAMES {
        let answer = create_domain(&gateway, json!({ "domainName": nth_name(index) })).await;
        assert_eq!(
            answer.status,
            201,
            "case create {index} of {MAX_DOMAIN_NAMES} (at the quota): expected 201, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // ONE PAST the bound: refused with the modelled failure, whatever shape the request
    // takes — a fresh name, a private record (whose key is minted and so could never
    // collide on a name), or a tagged one.
    for (case, body) in [
        (
            "one past the quota",
            json!({ "domainName": nth_name(MAX_DOMAIN_NAMES) }),
        ),
        (
            "one past the quota, as a private record",
            json!({
                "domainName": nth_name(MAX_DOMAIN_NAMES + 1),
                "endpointConfiguration": { "types": ["PRIVATE"] },
            }),
        ),
        (
            "one past the quota, tagged",
            json!({
                "domainName": nth_name(MAX_DOMAIN_NAMES + 2),
                "tags": { "team": "platform" },
            }),
        ),
    ] {
        let answer = create_domain(&gateway, body).await;
        assert_failure(&answer, 429, "LimitExceededException", case);
    }

    // A refused create changes nothing a later caller sees: the record at the bound is
    // still readable, and each refused name resolves to no record.
    let at_the_bound = nth_name(MAX_DOMAIN_NAMES - 1);
    let last = get_domain(&gateway, &at_the_bound).await;
    assert_eq!(
        last.status, 200,
        "case the record at the quota: expected it to still be readable, got {} with {:?}",
        last.status, last.body
    );
    assert_eq!(
        string_member(&last.body, "domainName", "the record at the quota"),
        at_the_bound,
        "case the record at the quota: expected the record its name addresses, got {:?}",
        last.body
    );
    for index in [MAX_DOMAIN_NAMES, MAX_DOMAIN_NAMES + 1, MAX_DOMAIN_NAMES + 2] {
        let refused = get_domain(&gateway, &nth_name(index)).await;
        assert_failure(
            &refused,
            404,
            "NotFoundException",
            &format!("the refused name {index}"),
        );
    }
}
