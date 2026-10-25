// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service VPC endpoint access surface:
//! `AuthorizeVpcEndpointAccess` and `ListVpcEndpointAccess`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the two
//! paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a grant persists the ONE principal its request names and the listing reports
//!   that same record, over a table of seven grants differing in which member names
//!   the principal (an account, a service), in the account id's length and digits, and
//!   in the service options carried with it (absent, empty, one Region, several);
//! * the principal type recorded is the one the model declares for the kind of
//!   principal named, and the table covers every value `PrincipalType` declares;
//! * the principal IS the identity of an authorization: granting the same principal
//!   again replaces its one record and leaves every other principal's alone;
//! * a request that names no principal, or both kinds at once, is refused and the
//!   stored set stands — as is a service principal outside the modelled enum;
//! * the set belongs to the domain the request names: a grant on one domain is
//!   invisible on another, a name the registry does not resolve is refused by both
//!   operations, and deleting a domain releases its authorizations with it;
//! * paging walks the whole set exactly once in the order the listing reports it, with
//!   every page but the last full, and a token that names no principal of the set is
//!   refused;
//! * the set is bounded per domain: grants are accepted up to the quota and the one
//!   past it is refused with the modelled `LimitExceededException`, while a re-grant of
//!   a principal already in the set is still accepted at the bound because it does not
//!   grow it.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    A_W_S_SERVICE_PRINCIPAL_VALUES, PRINCIPAL_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PRINCIPAL_TYPE_AWS_ACCOUNT, PRINCIPAL_TYPE_AWS_SERVICE, VPC_ENDPOINT_ACCESS_PAGE,
    VPC_ENDPOINT_ACCESS_QUOTA,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// `AuthorizeVpcEndpointAccess`:
/// `POST /opensearch/domain/{DomainName}/authorizeVpcEndpointAccess`.
async fn authorize(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/authorizeVpcEndpointAccess"),
            Some(body),
        )
        .await
}

/// `ListVpcEndpointAccess`: `GET /opensearch/domain/{DomainName}/listVpcEndpointAccess`,
/// optionally resumed from a page token.
async fn list(emulator: &Emulator, domain: &str, token: Option<&str>) -> Answer {
    let uri = match token {
        None => format!("{PREFIX}/opensearch/domain/{domain}/listVpcEndpointAccess"),
        Some(token) => {
            format!("{PREFIX}/opensearch/domain/{domain}/listVpcEndpointAccess?nextToken={token}")
        }
    };
    emulator.call("GET", &uri, None).await
}

/// A domain to grant access to, created through `CreateDomain` so the grants are made
/// against a domain that exists.
async fn created_domain(emulator: &Emulator, name: &str) {
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
}

/// The `AuthorizedPrincipalList` of a listing answer, with the case named on failure.
fn principal_list(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["AuthorizedPrincipalList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an AuthorizedPrincipalList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `NextToken` of a listing answer. The member is required by the model, so an
/// answer that carries no string at all is itself a failure; the end of the listing is
/// the EMPTY token.
fn next_token(answer: &Answer, case: &str) -> String {
    answer.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string NextToken, got {:?}", answer.body))
        .to_owned()
}

/// Every authorized principal of a domain, walked page by page through the tokens the
/// listing hands out — so a set larger than one page is still read in full.
async fn all_principals(emulator: &Emulator, domain: &str, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded by the pages the quota can fill, so a listing that never ends fails the
    // test instead of looping forever.
    let max_pages = VPC_ENDPOINT_ACCESS_QUOTA / VPC_ENDPOINT_ACCESS_PAGE + 2;
    for page in 0..=max_pages {
        assert!(
            page < max_pages,
            "case {case}: the listing handed out more than {max_pages} pages"
        );
        let answer = list(emulator, domain, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from page {page}, got {} with {:?}",
            answer.status, answer.body
        );
        let mut entries = principal_list(&answer, case);
        let carried = next_token(&answer, case);
        let last = carried.is_empty();
        // Every page but the last is FULL: a short page before the end would mean the
        // token skipped records.
        if !last {
            assert_eq!(
                entries.len(),
                VPC_ENDPOINT_ACCESS_PAGE,
                "case {case}: page {page} is short of the page size yet carries the token \
                 {carried:?}, got {entries:?}"
            );
        }
        collected.append(&mut entries);
        if last {
            break;
        }
        token = Some(carried);
    }
    collected
}

/// The `Principal` of one reported `AuthorizedPrincipal`.
fn principal_of(entry: &Value, case: &str) -> String {
    entry["Principal"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string Principal, got {entry:?}"))
        .to_owned()
}

/// One grant the table drives: the request body, and the principal and principal type
/// the service must record it under.
struct Grant {
    /// What tells this row from the others, for the case label.
    label: &'static str,
    /// The request body, which carries everything but the domain name (that is the
    /// path label).
    body: Value,
    /// `AuthorizedPrincipal$Principal` the grant must be recorded under.
    principal: String,
    /// `AuthorizedPrincipal$PrincipalType` it must be recorded with.
    principal_type: &'static str,
    /// The `ServiceOptions` the record must carry, or `None` where the request carried
    /// none and the answer must carry no such member either.
    service_options: Option<Value>,
}

/// The only value `AWSServicePrincipal` declares, from this service's Smithy model
/// (`smithy_query(query="shape",shape="AWSServicePrincipal")`, which the generated
/// `A_W_S_SERVICE_PRINCIPAL_VALUES` carries): a request naming any other service is
/// refused at the request boundary, so this is the one service a grant can name.
fn service_principal() -> &'static str {
    A_W_S_SERVICE_PRINCIPAL_VALUES[0]
}

/// The grant table: one row per way one accepted grant differs from another — which
/// member names the principal, the digits and length of an account id (`AWSAccount` is
/// `pattern ^[0-9]+$` with no length bound), and the service options carried with it.
fn grant_rows() -> Vec<Grant> {
    vec![
        Grant {
            label: "account/12-digit/no-options",
            body: json!({ "Account": "123456789012" }),
            principal: "123456789012".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: None,
        },
        Grant {
            label: "account/all-zero/empty-options",
            body: json!({ "Account": "000000000000", "ServiceOptions": {} }),
            principal: "000000000000".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({})),
        },
        Grant {
            label: "account/one-digit/one-region",
            body: json!({
                "Account": "7",
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
            principal: "7".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({ "SupportedRegions": ["us-east-1"] })),
        },
        Grant {
            label: "account/long/no-regions",
            body: json!({
                "Account": "98765432109876543210",
                "ServiceOptions": { "SupportedRegions": [] },
            }),
            principal: "98765432109876543210".to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_ACCOUNT,
            service_options: Some(json!({ "SupportedRegions": [] })),
        },
        Grant {
            label: "service/no-options",
            body: json!({ "Service": service_principal() }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: None,
        },
        Grant {
            label: "service/one-region",
            body: json!({
                "Service": service_principal(),
                "ServiceOptions": { "SupportedRegions": ["eu-west-1"] },
            }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: Some(json!({ "SupportedRegions": ["eu-west-1"] })),
        },
        Grant {
            label: "service/three-regions",
            body: json!({
                "Service": service_principal(),
                "ServiceOptions": {
                    "SupportedRegions": ["us-east-1", "eu-west-1", "ap-south-1"],
                },
            }),
            principal: service_principal().to_string(),
            principal_type: PRINCIPAL_TYPE_AWS_SERVICE,
            service_options: Some(json!({
                "SupportedRegions": ["us-east-1", "eu-west-1", "ap-south-1"],
            })),
        },
    ]
}

/// Assert one reported `AuthorizedPrincipal` is the record `grant` asked for.
fn assert_is_grant(entry: &Value, grant: &Grant, case: &str) {
    assert_eq!(
        entry["Principal"].as_str(),
        Some(grant.principal.as_str()),
        "case {case}: expected Principal {:?}, got {entry:?}",
        grant.principal
    );
    assert_eq!(
        entry["PrincipalType"].as_str(),
        Some(grant.principal_type),
        "case {case}: expected PrincipalType {:?}, got {entry:?}",
        grant.principal_type
    );
    // Whatever type is reported is one the model declares, never a value of this
    // emulator's own invention.
    let reported_type = entry["PrincipalType"].as_str().unwrap_or_default();
    assert!(
        PRINCIPAL_TYPE_VALUES.contains(&reported_type),
        "case {case}: PrincipalType {reported_type:?} is outside the modelled values \
         {PRINCIPAL_TYPE_VALUES:?}"
    );
    match grant.service_options.as_ref() {
        Some(expected) => assert_eq!(
            entry.get("ServiceOptions"),
            Some(expected),
            "case {case}: expected ServiceOptions {expected:?}, got {entry:?}"
        ),
        None => assert!(
            entry.get("ServiceOptions").is_none(),
            "case {case}: a grant that carried no ServiceOptions must report no such \
             member, got {entry:?}"
        ),
    }
}

#[tokio::test]
async fn a_grant_persists_the_principal_it_names_and_the_listing_reports_that_record() {
    for grant in grant_rows() {
        let case = format!("AuthorizeVpcEndpointAccess({})", grant.label);
        // A fresh emulator per row, so the row's grant is the whole set and the
        // listing's answer is about this row alone.
        let emulator = Emulator::new();
        created_domain(&emulator, "search-target").await;

        let granted = authorize(&emulator, "search-target", grant.body.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            granted.status, granted.body
        );
        assert_is_grant(&granted.body["AuthorizedPrincipal"], &grant, &case);

        // Read back through the implemented reader: the grant was persisted, and the
        // listing reports the SAME record the grant answered with.
        let listed = list(&emulator, "search-target", None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from the listing, got {} with {:?}",
            listed.status, listed.body
        );
        let entries = principal_list(&listed, &case);
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected exactly the granted principal, got {entries:?}"
        );
        assert_is_grant(&entries[0], &grant, &case);
        assert_eq!(
            entries[0], granted.body["AuthorizedPrincipal"],
            "case {case}: the listing must report the record the grant answered with, \
             got {:?} then {:?}",
            granted.body["AuthorizedPrincipal"], entries[0]
        );
        // One page holds the whole set, so the listing ends here: the required token is
        // the empty one.
        assert_eq!(
            next_token(&listed, &case),
            "",
            "case {case}: expected no further page, got {:?}",
            listed.body
        );

        // The listing reported the set and changed nothing: a second one answers
        // exactly the same.
        let again = list(&emulator, "search-target", None).await;
        assert_eq!(
            again.body, listed.body,
            "case {case}: a read must not change what the next caller sees, got {:?} then {:?}",
            listed.body, again.body
        );
    }
}

#[tokio::test]
async fn every_principal_type_the_model_declares_is_recorded_by_some_grant() {
    let covered: Vec<&str> = grant_rows()
        .iter()
        .map(|grant| grant.principal_type)
        .collect();
    for principal_type in PRINCIPAL_TYPE_VALUES {
        assert!(
            covered.contains(principal_type),
            "case PrincipalType({principal_type}): no row of the grant table records it, \
             covered {covered:?}"
        );
    }
}

#[tokio::test]
async fn granting_a_principal_again_replaces_its_one_record_and_leaves_the_others() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-shared").await;

    // One grant per DISTINCT principal of the table, in table order, so the set ends up
    // holding exactly the distinct principals however many rows named each.
    let rows = grant_rows();
    for grant in &rows {
        let case = format!("AuthorizeVpcEndpointAccess({})", grant.label);
        let granted = authorize(&emulator, "search-shared", grant.body.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            granted.status, granted.body
        );
        assert_is_grant(&granted.body["AuthorizedPrincipal"], grant, &case);
    }

    let mut distinct: Vec<String> = rows.iter().map(|grant| grant.principal.clone()).collect();
    distinct.sort();
    distinct.dedup();

    let listed = all_principals(&emulator, "search-shared", "re-grant").await;
    let reported: Vec<String> = listed
        .iter()
        .map(|entry| principal_of(entry, "re-grant"))
        .collect();
    assert_eq!(
        reported, distinct,
        "case re-grant: a repeated grant must replace one record rather than add a \
         second, expected {distinct:?}, got {reported:?}"
    );

    // The record each principal is left with is the LAST grant of it: the service
    // principal was granted three times, and the options of the third stand.
    let last_service_grant = rows
        .iter()
        .rfind(|grant| grant.principal_type == PRINCIPAL_TYPE_AWS_SERVICE)
        .expect("the table grants a service principal");
    let entry = listed
        .iter()
        .find(|entry| principal_of(entry, "re-grant") == last_service_grant.principal)
        .expect("the service principal is reported");
    assert_is_grant(entry, last_service_grant, "re-grant/last-wins");
}

#[tokio::test]
async fn a_request_that_names_no_principal_or_two_is_refused_and_the_set_stands() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-guarded").await;

    // One accepted grant, so a refused request has something to leave untouched.
    let standing = grant_rows().remove(0);
    authorize(&emulator, "search-guarded", standing.body.clone()).await;

    let refused: Vec<(&str, Value)> = vec![
        // No principal at all: nothing to grant access to.
        ("no-principal", json!({})),
        // No principal, yet options for one.
        (
            "options-only",
            json!({ "ServiceOptions": { "SupportedRegions": ["us-east-1"] } }),
        ),
        // Two principals: one call grants access to one principal.
        (
            "both-kinds",
            json!({ "Account": "123456789012", "Service": service_principal() }),
        ),
        (
            "both-kinds-with-options",
            json!({
                "Account": "210987654321",
                "Service": service_principal(),
                "ServiceOptions": { "SupportedRegions": ["us-east-1"] },
            }),
        ),
        // A service principal outside the modelled enum, refused at the request
        // boundary.
        ("unmodelled-service", json!({ "Service": "ec2.amazonaws.com" })),
        // An account id the modelled pattern refuses.
        ("non-numeric-account", json!({ "Account": "12345678901a" })),
    ];

    for (label, body) in refused {
        let case = format!("AuthorizeVpcEndpointAccess({label})");
        let answer = authorize(&emulator, "search-guarded", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );

        // A refused grant stored nothing: the set still holds exactly the accepted one.
        let listed = list(&emulator, "search-guarded", None).await;
        let entries = principal_list(&listed, &case);
        assert_eq!(
            entries.len(),
            1,
            "case {case}: a refused grant must leave the set as it was, got {entries:?}"
        );
        assert_is_grant(&entries[0], &standing, &case);
    }
}

#[tokio::test]
async fn the_authorization_set_belongs_to_the_domain_the_request_names() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-one").await;
    created_domain(&emulator, "search-two").await;

    let grant = grant_rows().remove(0);
    authorize(&emulator, "search-one", grant.body.clone()).await;

    // Another domain of the same account holds its own set, which this grant is not in.
    let other = list(&emulator, "search-two", None).await;
    assert_eq!(
        other.status, 200,
        "case ListVpcEndpointAccess(search-two): expected 200, got {} with {:?}",
        other.status, other.body
    );
    assert!(
        principal_list(&other, "other-domain").is_empty(),
        "case ListVpcEndpointAccess(search-two): a domain nobody was granted access to \
         must report an empty list, got {:?}",
        other.body
    );

    // A name the registry does not resolve has no set to grant into or to report.
    for (label, answer) in [
        (
            "authorize",
            authorize(&emulator, "search-absent", grant.body.clone()).await,
        ),
        ("list", list(&emulator, "search-absent", None).await),
    ] {
        let case = format!("{label}(search-absent)");
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409 for a domain the registry does not hold, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // An authorization is access TO a domain, so it goes when the domain does.
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/search-one"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(search-one): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );
    let after = list(&emulator, "search-one", None).await;
    assert_eq!(
        error_code(&after),
        "ResourceNotFoundException",
        "case ListVpcEndpointAccess(search-one after delete): expected \
         ResourceNotFoundException, got {:?}",
        after.body
    );
}

#[tokio::test]
async fn paging_walks_the_whole_set_exactly_once_and_an_unresolvable_token_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-paged").await;

    // More principals than one page holds, so the set spans pages.
    let count = VPC_ENDPOINT_ACCESS_PAGE + 3;
    let mut granted: Vec<String> = Vec::new();
    for index in 0..count {
        let account = format!("{:012}", index + 1);
        let case = format!("AuthorizeVpcEndpointAccess({account})");
        let answer = authorize(&emulator, "search-paged", json!({ "Account": account })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        granted.push(account);
    }
    granted.sort();

    let walked = all_principals(&emulator, "search-paged", "paging").await;
    let reported: Vec<String> = walked
        .iter()
        .map(|entry| principal_of(entry, "paging"))
        .collect();
    assert_eq!(
        walked.len(),
        count,
        "case paging: expected {count} principals across the pages, got {reported:?}"
    );
    assert_eq!(
        reported, granted,
        "case paging: paging must walk the whole set exactly once, expected {granted:?}, \
         got {reported:?}"
    );

    // The token of the first page names where the second starts, so resuming from it
    // skips exactly the first page and nothing else.
    let first = list(&emulator, "search-paged", None).await;
    let token = next_token(&first, "paging/first");
    assert_eq!(
        token, granted[VPC_ENDPOINT_ACCESS_PAGE],
        "case paging/first: the token must name the principal the next page starts at, \
         got {:?}",
        first.body
    );
    let second = list(&emulator, "search-paged", Some(&token)).await;
    let tail = principal_list(&second, "paging/second");
    assert_eq!(
        tail.len(),
        count - VPC_ENDPOINT_ACCESS_PAGE,
        "case paging/second: expected the rest of the set, got {tail:?}"
    );

    // The token the LAST page hands out is the empty one, and echoing it back is not a
    // refusal: it starts the listing from the beginning, exactly as naming no token
    // does.
    let echoed = list(&emulator, "search-paged", Some("")).await;
    assert_eq!(
        echoed.status, 200,
        "case ListVpcEndpointAccess(nextToken=): expected 200 for the terminal token, \
         got {} with {:?}",
        echoed.status, echoed.body
    );
    assert_eq!(
        principal_list(&echoed, "paging/echoed"),
        principal_list(&first, "paging/first"),
        "case ListVpcEndpointAccess(nextToken=): expected the first page back, got {:?}",
        echoed.body
    );

    // A token this listing never handed out names no place in the set.
    for label in ["000000000000", "not-a-token", "99999999999999999999"] {
        let case = format!("ListVpcEndpointAccess(nextToken={label})");
        let answer = list(&emulator, "search-paged", Some(label)).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 for a token naming no place in the set, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_authorization_set_is_bounded_per_domain() {
    let emulator = Emulator::new();
    created_domain(&emulator, "search-bounded").await;
    created_domain(&emulator, "search-spare").await;

    for index in 0..VPC_ENDPOINT_ACCESS_QUOTA {
        let account = format!("{:012}", index + 1);
        let case = format!("AuthorizeVpcEndpointAccess({account})");
        let answer = authorize(&emulator, "search-bounded", json!({ "Account": account })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 inside the quota, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // One principal past the quota is refused rather than stored.
    let past = authorize(
        &emulator,
        "search-bounded",
        json!({ "Service": service_principal() }),
    )
    .await;
    assert_eq!(
        past.status, 409,
        "case AuthorizeVpcEndpointAccess(past the quota): expected 409, got {} with {:?}",
        past.status, past.body
    );
    assert_eq!(
        error_code(&past),
        "LimitExceededException",
        "case AuthorizeVpcEndpointAccess(past the quota): expected LimitExceededException, \
         got {:?}",
        past.body
    );

    let held = all_principals(&emulator, "search-bounded", "bounded").await;
    assert_eq!(
        held.len(),
        VPC_ENDPOINT_ACCESS_QUOTA,
        "case bounded: a refused grant must leave the set at the quota, got {} entries",
        held.len()
    );

    // A re-grant of a principal already in the set does not grow it, so the bound does
    // not refuse it.
    let regrant = authorize(
        &emulator,
        "search-bounded",
        json!({
            "Account": "000000000001",
            "ServiceOptions": { "SupportedRegions": ["us-west-2"] },
        }),
    )
    .await;
    assert_eq!(
        regrant.status, 200,
        "case AuthorizeVpcEndpointAccess(re-grant at the quota): expected 200, got {} \
         with {:?}",
        regrant.status, regrant.body
    );
    assert_eq!(
        regrant.body["AuthorizedPrincipal"]["ServiceOptions"],
        json!({ "SupportedRegions": ["us-west-2"] }),
        "case AuthorizeVpcEndpointAccess(re-grant at the quota): the re-grant must \
         replace the record, got {:?}",
        regrant.body
    );
    let after = all_principals(&emulator, "search-bounded", "bounded/re-grant").await;
    assert_eq!(
        after.len(),
        VPC_ENDPOINT_ACCESS_QUOTA,
        "case bounded/re-grant: a re-grant must not grow the set, got {} entries",
        after.len()
    );

    // The quota partitions by DOMAIN: another domain's set is empty and takes a grant.
    let spare = authorize(
        &emulator,
        "search-spare",
        json!({ "Service": service_principal() }),
    )
    .await;
    assert_eq!(
        spare.status, 200,
        "case AuthorizeVpcEndpointAccess(search-spare): a full domain must not exhaust \
         another's allowance, got {} with {:?}",
        spare.status, spare.body
    );
}
