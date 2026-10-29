// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListVpcEndpointsForDomain`: which endpoints the listing of ONE
//! domain reports, and how it refuses a request it cannot resolve.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the create and domain helpers from [`super::vpc_endpoints`]; this suite adds only
//! the per-domain listing call itself.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports exactly the endpoints whose domain is the one named, over a
//!   table of domains holding none, one and several endpoints, with a second domain's
//!   endpoints never reported for the first;
//! * every summary it reports is the SAME stored record the account-wide listing reports,
//!   including the status the clock has left the endpoint in;
//! * a domain name this account holds no domain under selects no endpoint, so the
//!   listing is EMPTY rather than refused;
//! * one answer reports a domain's whole set exactly once and carries no `NextToken`
//!   member at all, and a token that names no endpoint of that domain is refused
//!   exactly as the account-wide listing refuses one.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::VPC_ENDPOINT_STATUS_ACTIVE;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::vpc_endpoints::{create_endpoint, member, minted, vpc_domain};

/// `ListVpcEndpointsForDomain`: `GET /2021-01-01/opensearch/domain/{DomainName}/vpcEndpoints`,
/// optionally resumed from a page token.
///
/// `pub(crate)` because the sibling suite for what status this listing reports calls THIS
/// wrapper rather than growing a second copy of it.
pub(crate) async fn list_for_domain(
    emulator: &Emulator,
    domain: &str,
    token: Option<&str>,
) -> Answer {
    let uri = match token {
        None => format!("{PREFIX}/opensearch/domain/{domain}/vpcEndpoints"),
        Some(token) => {
            format!("{PREFIX}/opensearch/domain/{domain}/vpcEndpoints?nextToken={token}")
        }
    };
    emulator.call("GET", &uri, None).await
}

/// One endpoint on the domain `arn` names, answering with its id.
///
/// `pub(crate)` for the same reason as [`list_for_domain`].
pub(crate) async fn endpoint_on(emulator: &Emulator, arn: &str, subnet: &str) -> String {
    let case = "an endpoint of the domain being listed";
    let answer = create_endpoint(
        emulator,
        json!({
            "DomainArn": arn,
            "VpcOptions": { "SubnetIds": [subnet] },
        }),
    )
    .await;
    member(&minted(&answer, case), "VpcEndpointId", case)
}

/// The ids the listing of `domain` reports, in the order it reports them.
///
/// `pub(crate)` for the same reason as [`list_for_domain`].
pub(crate) async fn listed_ids(emulator: &Emulator, domain: &str, case: &str) -> Vec<String> {
    let answer = list_for_domain(emulator, domain, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    summary_ids(&answer.body, case)
}

/// The `VpcEndpointId`s of a page's `VpcEndpointSummaryList`.
fn summary_ids(body: &Value, case: &str) -> Vec<String> {
    body["VpcEndpointSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a VpcEndpointSummaryList, got {body:?}"))
        .iter()
        .map(|summary| {
            summary["VpcEndpointId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a summary with an id, got {body:?}")
                })
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_exactly_the_endpoints_of_the_domain_it_names() {
    let emulator = Emulator::new();

    // One row per size of a domain's endpoint set: none, one, and several. Each row's
    // domain is created beside the others, so what the listing must leave out — every
    // other domain's endpoints — is present in the store throughout.
    let empty = "vpce-domain-none";
    let single = "vpce-domain-one";
    let several = "vpce-domain-several";

    let _empty_arn = vpc_domain(&emulator, empty).await;
    let single_arn = vpc_domain(&emulator, single).await;
    let several_arn = vpc_domain(&emulator, several).await;

    let on_single = endpoint_on(&emulator, &single_arn, "subnet-0aaa0000000000001").await;
    let mut on_several = Vec::new();
    for index in 0..3 {
        on_several.push(
            endpoint_on(
                &emulator,
                &several_arn,
                &format!("subnet-0bbb000000000000{index}"),
            )
            .await,
        );
    }

    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("a domain with no endpoint", empty, Vec::new()),
        ("a domain with one endpoint", single, vec![on_single.clone()]),
        ("a domain with several endpoints", several, on_several.clone()),
    ];

    for (case, domain, expected) in &rows {
        let mut reported = listed_ids(&emulator, domain, case).await;
        let mut wanted = expected.clone();
        reported.sort();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case}: the listing reports exactly this domain's endpoints"
        );
    }

    // The domain the ARN of each endpoint names is the only domain that reports it: no
    // listing above carries another row's endpoint.
    let on_empty = listed_ids(&emulator, empty, "a domain with no endpoint").await;
    assert!(
        on_empty.is_empty(),
        "case a domain with no endpoint: expected an empty page, got {on_empty:?}"
    );

    // Every summary is the SAME record the account-wide listing reports, status
    // included: the two listings page one set of records.
    tokio::time::sleep(SETTLE_WAIT).await;
    let account_wide = emulator
        .call("GET", &format!("{PREFIX}/opensearch/vpcEndpoints"), None)
        .await;
    assert_eq!(
        account_wide.status, 200,
        "case ListVpcEndpoints: expected 200, got {} with {:?}",
        account_wide.status, account_wide.body
    );
    let all = account_wide.body["VpcEndpointSummaryList"]
        .as_array()
        .expect("the account-wide listing reports a summary list")
        .clone();
    let per_domain = list_for_domain(&emulator, several, None).await;
    for summary in per_domain.body["VpcEndpointSummaryList"]
        .as_array()
        .expect("the per-domain listing reports a summary list")
    {
        assert!(
            all.contains(summary),
            "case a domain with several endpoints: the per-domain summary is the record the \
             account-wide listing reports, got {summary:?} against {all:?}"
        );
        assert_eq!(
            summary["Status"],
            json!(VPC_ENDPOINT_STATUS_ACTIVE),
            "case a domain with several endpoints: the status the clock owed is reported once the \
             span passed, got {summary:?}"
        );
        assert_eq!(
            summary["DomainArn"],
            json!(several_arn),
            "case a domain with several endpoints: every endpoint names this domain, got \
             {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_name_that_names_no_domain_reports_an_empty_listing() {
    let emulator = Emulator::new();
    let gone = "vpce-domain-deleted";
    vpc_domain(&emulator, gone).await;
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{gone}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain({gone:?}): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    // One endpoint exists throughout, on a different domain: an empty answer must be the
    // selection reporting nothing, not the store holding nothing.
    let held = vpc_domain(&emulator, "vpce-domain-holder").await;
    endpoint_on(&emulator, &held, "subnet-0eee0000000000001").await;

    // One row per way a name can name no domain of this account: a name nothing created,
    // a domain that was created and then released, and a well-formed name no create has
    // ever used.
    for (case, domain) in [
        ("a domain nothing created", "vpce-domain-absent"),
        ("a domain that was deleted", gone),
        ("another unused name", "vpce-domain-unused-name"),
    ] {
        let answer = list_for_domain(&emulator, domain, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["VpcEndpointSummaryList"],
            json!([]),
            "case {case:?}: a name that names no domain selects no endpoint, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.get("NextToken"),
            None,
            "case {case:?}: an empty listing is a complete result set, so it carries no \
             NextToken, got {:?}",
            answer.body
        );
    }

    // And the domain that DOES hold one still reports it, so the rows above report
    // nothing because of their selection and not because the listing reports nothing.
    let holder = listed_ids(
        &emulator,
        "vpce-domain-holder",
        "a domain holding one endpoint",
    )
    .await;
    assert_eq!(
        holder.len(),
        1,
        "case a domain holding one endpoint: expected its one endpoint, got {holder:?}"
    );
}

#[tokio::test]
async fn one_answer_reports_one_domain_s_set_exactly_once() {
    let emulator = Emulator::new();
    let domain = "vpce-domain-paged";
    let arn = vpc_domain(&emulator, domain).await;
    let other = vpc_domain(&emulator, "vpce-domain-beside").await;

    // Far more endpoints than the ten summaries this listing used to cap an answer at,
    // so a capped answer could not pass; plus a second domain's endpoint that the
    // listing may not carry.
    let total = 12;
    let mut minted = Vec::new();
    for index in 0..total {
        minted.push(endpoint_on(&emulator, &arn, &format!("subnet-0ccc{index:013}")).await);
    }
    let beside = endpoint_on(&emulator, &other, "subnet-0ddd0000000000001").await;

    let answer = list_for_domain(&emulator, domain, None).await;
    assert_eq!(
        answer.status, 200,
        "case one domain's set: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    // A complete result set is reported with NO `NextToken` member at all — not the
    // empty string, which a client testing for the member's presence rather than its
    // truthiness paginates on forever.
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case one domain's set: the whole set is reported, so the answer carries no NextToken, \
         got {:?}",
        answer.body
    );
    let listed = summary_ids(&answer.body, "one domain's set");

    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "case one domain's set: one answer reports every endpoint of the domain exactly once, in \
         the order the listing reports them"
    );
    assert!(
        !listed.contains(&beside),
        "case one domain's set: the answer carries no other domain's endpoint, got {listed:?}"
    );

    // A token that names no endpoint OF THIS DOMAIN names no place in its listing — a
    // token this service never handed out, and one naming an endpoint of another domain.
    // The refusal is the one the account-wide listing states for the same condition.
    let account_wide_refusal = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/vpcEndpoints?nextToken=aos-notatoken"),
            None,
        )
        .await;
    assert_eq!(
        error_code(&account_wide_refusal),
        "ValidationException",
        "case ListVpcEndpoints with a token it never issued: expected ValidationException, got \
         {:?}",
        account_wide_refusal.body
    );
    let stated = account_wide_refusal.body["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    for (case, bad) in [
        ("a token this service never issued", "aos-notatoken"),
        ("a token naming another domain's endpoint", beside.as_str()),
    ] {
        let answer = list_for_domain(&emulator, domain, Some(bad)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(stated.as_str()),
            "case {case:?}: the refusal states the pagination-token condition in the words both \
             listings use, got {:?}",
            answer.body
        );
    }
}
