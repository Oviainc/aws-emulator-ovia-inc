// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OUTBOUND cross-cluster search connection:
//! `CreateOutboundCrossClusterSearchConnection` and the
//! `DescribeOutboundCrossClusterSearchConnections` that reads it back, driven as real
//! restJson1 requests through the router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A create PERSISTS the connection: the response is the projection of the stored
//!    record, and the describe answers the same members for the same id.
//! 2. A reference naming no `OwnerId` is to the calling account's domain, and one
//!    naming no `Region` is to a domain in the Region the call was signed for; both
//!    are reported in full however little the request said.
//! 3. The domains are validated ASYNCHRONOUSLY: every create answers `VALIDATING`
//!    with no status message, and once the validation span has passed the connection
//!    holds `PENDING_ACCEPTANCE` when both references resolve to a stored domain and
//!    `VALIDATION_FAILED`, naming the domains that did not resolve, when one does not.
//! 4. One source domain holds at most one connection per alias; the same alias on
//!    another source domain is a different connection.
//! 5. The describe selects on each filter name the model documents, cumulatively
//!    across filters and alternatively within one filter's values, and on no other
//!    name; it pages by `MaxResults` and resumes at `NextToken`, and refuses a token
//!    that names no place in the set.
//! 6. A connection belongs to the Region its create was signed for.
//! 7. Reading a connection changes nothing a later read sees.
//! 8. One source domain holds at most the documented number of connections, and a
//!    create past that bound stores nothing.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the tests sign for, and the one a reference naming no Region of its own
/// resolves against.
const REGION: &str = "us-west-2";

/// Another region, used to prove a connection belongs to the Region it was created
/// in and that a destination elsewhere is recorded as given.
const OTHER_REGION: &str = "eu-west-1";

/// Longer than the emulator's validation span, so the outcome is certainly due.
const PAST_VALIDATION: Duration = Duration::from_millis(3_500);

async fn create_domain_in(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({"DomainName": name})),
    )
    .await
}

async fn create_connection_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/2015-01-01/es/ccs/outboundConnection",
        Some(body),
    )
    .await
}

async fn describe_connections_in(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/2015-01-01/es/ccs/outboundConnection/search",
        Some(body),
    )
    .await
}

/// The connection with `id` in a describe answer, or `None`.
fn connection_with(answer: &Answer, id: &str) -> Option<Value> {
    answer
        .body
        .pointer("/CrossClusterSearchConnections")?
        .as_array()?
        .iter()
        .find(|connection| {
            connection
                .pointer("/CrossClusterSearchConnectionId")
                .and_then(Value::as_str)
                == Some(id)
        })
        .cloned()
}

/// Every connection id a describe answer reports, in the order it reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    answer
        .body
        .pointer("/CrossClusterSearchConnections")
        .and_then(Value::as_array)
        .map(|connections| {
            connections
                .iter()
                .filter_map(|connection| {
                    connection
                        .pointer("/CrossClusterSearchConnectionId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One create request and what the store must then answer for it.
struct ConnectionCase {
    label: &'static str,
    /// Domains to create in [`REGION`] before the connection request.
    existing_domains: &'static [&'static str],
    request: Value,
    /// The alias the connection must report.
    alias: &'static str,
    /// `OwnerId` / `Region` the source reference must be reported with.
    source_owner: String,
    source_region: &'static str,
    /// `OwnerId` / `Region` the destination reference must be reported with.
    destination_owner: String,
    destination_region: &'static str,
    /// The status the connection holds once the validation span has passed.
    settled_status: &'static str,
    /// The domain names the settled status message must name, in order — empty when
    /// the settled status carries no message.
    unresolved: &'static [&'static str],
}

/// Rows differ along what rules 2 and 3 generalise over: which references resolve,
/// how much of a reference the request spells out, which account and Region it names,
/// and the alias's length.
fn connection_cases() -> Vec<ConnectionCase> {
    let account = engine::emulator_account_id().to_string();
    vec![
        ConnectionCase {
            label: "both domains exist, references spelled out in full",
            existing_domains: &["ccs-source-one", "ccs-destination-one"],
            request: json!({
                "ConnectionAlias": "both-exist",
                "SourceDomainInfo": {
                    "DomainName": "ccs-source-one",
                    "OwnerId": "000000000000",
                    "Region": REGION
                },
                "DestinationDomainInfo": {
                    "DomainName": "ccs-destination-one",
                    "OwnerId": "000000000000",
                    "Region": REGION
                }
            }),
            alias: "both-exist",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "PENDING_ACCEPTANCE",
            unresolved: &[],
        },
        ConnectionCase {
            label: "both domains exist, bare references taking account and Region from the call",
            existing_domains: &["ccs-source-two", "ccs-destination-two"],
            request: json!({
                "ConnectionAlias": "bare-refs",
                "SourceDomainInfo": {"DomainName": "ccs-source-two"},
                "DestinationDomainInfo": {"DomainName": "ccs-destination-two"}
            }),
            alias: "bare-refs",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "PENDING_ACCEPTANCE",
            unresolved: &[],
        },
        ConnectionCase {
            label: "destination names no domain at all",
            existing_domains: &["ccs-source-three"],
            request: json!({
                "ConnectionAlias": "no-destination",
                "SourceDomainInfo": {"DomainName": "ccs-source-three"},
                "DestinationDomainInfo": {"DomainName": "ccs-absent-destination"}
            }),
            alias: "no-destination",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "VALIDATION_FAILED",
            unresolved: &["ccs-absent-destination"],
        },
        ConnectionCase {
            label: "source names no domain at all",
            existing_domains: &["ccs-destination-four"],
            request: json!({
                "ConnectionAlias": "no-source",
                "SourceDomainInfo": {"DomainName": "ccs-absent-source"},
                "DestinationDomainInfo": {"DomainName": "ccs-destination-four"}
            }),
            alias: "no-source",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "VALIDATION_FAILED",
            unresolved: &["ccs-absent-source"],
        },
        ConnectionCase {
            label: "neither domain exists, and the alias is at its 20-character maximum",
            existing_domains: &[],
            request: json!({
                "ConnectionAlias": "twenty-chars-alias01",
                "SourceDomainInfo": {"DomainName": "ccs-absent-src-five"},
                "DestinationDomainInfo": {"DomainName": "ccs-absent-dst-five"}
            }),
            alias: "twenty-chars-alias01",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "VALIDATION_FAILED",
            unresolved: &["ccs-absent-src-five", "ccs-absent-dst-five"],
        },
        ConnectionCase {
            label: "both references name the SAME absent domain, with an empty alias",
            existing_domains: &[],
            request: json!({
                "ConnectionAlias": "",
                "SourceDomainInfo": {"DomainName": "ccs-absent-both-six"},
                "DestinationDomainInfo": {"DomainName": "ccs-absent-both-six"}
            }),
            alias: "",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: REGION,
            settled_status: "VALIDATION_FAILED",
            unresolved: &["ccs-absent-both-six"],
        },
        ConnectionCase {
            label: "destination in another Region, where this account holds no such domain",
            existing_domains: &["ccs-source-seven", "ccs-destination-seven"],
            request: json!({
                "ConnectionAlias": "other-region-dst",
                "SourceDomainInfo": {"DomainName": "ccs-source-seven"},
                "DestinationDomainInfo": {
                    "DomainName": "ccs-destination-seven",
                    "Region": OTHER_REGION
                }
            }),
            alias: "other-region-dst",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: account.clone(),
            destination_region: OTHER_REGION,
            settled_status: "VALIDATION_FAILED",
            unresolved: &["ccs-destination-seven"],
        },
        ConnectionCase {
            label: "destination owned by another account, which is recorded as given",
            existing_domains: &["ccs-source-eight", "ccs-destination-eight"],
            request: json!({
                "ConnectionAlias": "foreign-owner",
                "SourceDomainInfo": {"DomainName": "ccs-source-eight"},
                "DestinationDomainInfo": {
                    "DomainName": "ccs-destination-eight",
                    "OwnerId": "210987654321"
                }
            }),
            alias: "foreign-owner",
            source_owner: account.clone(),
            source_region: REGION,
            destination_owner: "210987654321".to_string(),
            destination_region: REGION,
            settled_status: "PENDING_ACCEPTANCE",
            unresolved: &[],
        },
    ]
}

/// Rules 1, 2, 3 and 7: every create persists a connection the describe reads back,
/// reports it as still `VALIDATING`, and settles it on the outcome the referenced
/// domains earn once the validation span has passed.
#[tokio::test]
async fn a_created_connection_is_read_back_and_validated_asynchronously() {
    let router = router();
    let cases = connection_cases();
    let mut created: Vec<(String, String)> = Vec::new();

    for case in &cases {
        for domain in case.existing_domains {
            let answer = create_domain_in(&router, REGION, domain).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: creating domain {domain} must succeed, got {body}",
                label = case.label,
                body = answer.body
            );
        }

        let answer = create_connection_in(&router, REGION, &case.request).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: create must succeed, got {status} {body}",
            label = case.label,
            status = answer.status,
            body = answer.body
        );

        let id = answer.text(case.label, "/CrossClusterSearchConnectionId");
        assert!(
            !id.is_empty(),
            "case {label:?}: the connection must carry an id, got {id:?}",
            label = case.label
        );
        assert!(
            created.iter().all(|(each, _)| each != &id),
            "case {label:?}: id {id:?} was already minted for another connection",
            label = case.label
        );

        assert_eq!(
            answer.at(case.label, "/ConnectionAlias"),
            &json!(case.alias),
            "case {label:?}: the alias must be the one the request gave",
            label = case.label
        );
        assert_eq!(
            answer.at(case.label, "/SourceDomainInfo"),
            &json!({
                "DomainName": case.request["SourceDomainInfo"]["DomainName"],
                "OwnerId": case.source_owner,
                "Region": case.source_region,
            }),
            "case {label:?}: the source reference must be reported in full",
            label = case.label
        );
        assert_eq!(
            answer.at(case.label, "/DestinationDomainInfo"),
            &json!({
                "DomainName": case.request["DestinationDomainInfo"]["DomainName"],
                "OwnerId": case.destination_owner,
                "Region": case.destination_region,
            }),
            "case {label:?}: the destination reference must be reported in full",
            label = case.label
        );

        // The validation is ASYNCHRONOUS: the create cannot already know its outcome.
        assert_eq!(
            answer.text(case.label, "/ConnectionStatus/StatusCode"),
            "VALIDATING",
            "case {label:?}: a create must answer VALIDATING, got {body}",
            label = case.label,
            body = answer.body
        );
        assert!(
            answer.missing("/ConnectionStatus/Message"),
            "case {label:?}: a request still being validated has no status message, got {body}",
            label = case.label,
            body = answer.body
        );

        // Rule 1: the describe answers the stored record, member for member.
        let describe = describe_connections_in(&router, REGION, &json!({})).await;
        assert_eq!(
            describe.status, 200,
            "case {label:?}: describe must succeed, got {body}",
            label = case.label,
            body = describe.body
        );
        let reported = connection_with(&describe, &id).unwrap_or_else(|| {
            panic!(
                "case {label:?}: describe does not report connection {id}, got {body}",
                label = case.label,
                body = describe.body
            )
        });
        for member in [
            "ConnectionAlias",
            "SourceDomainInfo",
            "DestinationDomainInfo",
            "ConnectionStatus",
        ] {
            assert_eq!(
                reported.pointer(&format!("/{member}")),
                answer.body.pointer(&format!("/{member}")),
                "case {label:?}: describe and create must report the same {member}",
                label = case.label
            );
        }

        created.push((id, case.label.to_string()));
    }

    // Rule 3: once the span has passed, every connection holds the outcome its
    // references earned — and rule 7: reading it twice gives the same answer.
    tokio::time::sleep(PAST_VALIDATION).await;
    let settled = describe_connections_in(&router, REGION, &json!({})).await;
    let again = describe_connections_in(&router, REGION, &json!({})).await;
    assert_eq!(
        settled.body, again.body,
        "two reads of the same connections must agree: reading must not write"
    );

    for (case, (id, _)) in cases.iter().zip(&created) {
        let reported = connection_with(&settled, id).unwrap_or_else(|| {
            panic!(
                "case {label:?}: describe does not report connection {id}, got {body}",
                label = case.label,
                body = settled.body
            )
        });
        let status = reported
            .pointer("/ConnectionStatus/StatusCode")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            status, case.settled_status,
            "case {label:?}: expected {expected} once validated, got {reported}",
            label = case.label,
            expected = case.settled_status
        );
        let message = reported
            .pointer("/ConnectionStatus/Message")
            .and_then(Value::as_str);
        if case.unresolved.is_empty() {
            assert!(
                message.is_none(),
                "case {label:?}: a validated connection carries no failure message, got \
                 {message:?}",
                label = case.label
            );
        } else {
            let message = message.unwrap_or_else(|| {
                panic!(
                    "case {label:?}: a failed validation must say why, got {reported}",
                    label = case.label
                )
            });
            for domain in case.unresolved {
                assert!(
                    message.contains(domain),
                    "case {label:?}: the message must name the unresolved domain {domain}, got \
                     {message:?}",
                    label = case.label
                );
            }
        }
    }
}

/// Rule 4: the alias is unique per source domain, and only per source domain.
#[tokio::test]
async fn an_alias_is_unique_per_source_domain() {
    let router = router();
    for domain in ["alias-source-a", "alias-source-b", "alias-destination"] {
        create_domain_in(&router, REGION, domain).await;
    }

    let connection = |source: &str, alias: &str| {
        json!({
            "ConnectionAlias": alias,
            "SourceDomainInfo": {"DomainName": source},
            "DestinationDomainInfo": {"DomainName": "alias-destination"}
        })
    };

    let first = create_connection_in(&router, REGION, &connection("alias-source-a", "shared")).await;
    assert_eq!(
        first.status, 200,
        "case \"first connection\": must succeed, got {body}",
        body = first.body
    );

    let cases: Vec<(&str, &str, &str, u16, &str)> = vec![
        (
            "same source domain, same alias",
            "alias-source-a",
            "shared",
            409,
            "ResourceAlreadyExistsException",
        ),
        (
            "same source domain, another alias",
            "alias-source-a",
            "other-alias",
            200,
            "",
        ),
        (
            "another source domain, same alias",
            "alias-source-b",
            "shared",
            200,
            "",
        ),
    ];
    for (case, source, alias, status, error_type) in cases {
        let answer = create_connection_in(&router, REGION, &connection(source, alias)).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {actual} {body}",
            actual = answer.status,
            body = answer.body
        );
        assert!(
            answer.error_type.contains(error_type),
            "case {case:?}: expected error type {error_type:?}, got {actual:?}",
            actual = answer.error_type
        );
        if status == 409 {
            let message = answer.text(case, "/message");
            for expected in ["shared", "alias-source-a"] {
                assert!(
                    message.contains(expected),
                    "case {case:?}: the refusal must name {expected:?}, got {message:?}"
                );
            }
        }
    }

    // The duplicate alias left the stored set as it was: three connections, not four.
    let describe = describe_connections_in(&router, REGION, &json!({})).await;
    assert_eq!(
        reported_ids(&describe).len(),
        3,
        "a refused create must store nothing, got {body}",
        body = describe.body
    );
}

/// Rule 5: the describe selects on every documented filter name and on no other, and
/// pages the selected set.
#[tokio::test]
async fn filters_select_and_pages_resume_where_the_last_one_ended() {
    let router = router();
    for domain in ["filter-source-a", "filter-source-b", "filter-destination"] {
        create_domain_in(&router, REGION, domain).await;
    }

    // Three connections differing in source domain, destination owner and
    // destination Region, so each filter name selects a different subset.
    let requests = [
        json!({
            "ConnectionAlias": "from-a-local",
            "SourceDomainInfo": {"DomainName": "filter-source-a"},
            "DestinationDomainInfo": {"DomainName": "filter-destination"}
        }),
        json!({
            "ConnectionAlias": "from-a-foreign",
            "SourceDomainInfo": {"DomainName": "filter-source-a"},
            "DestinationDomainInfo": {
                "DomainName": "filter-destination",
                "OwnerId": "210987654321",
                "Region": OTHER_REGION
            }
        }),
        json!({
            "ConnectionAlias": "from-b-local",
            "SourceDomainInfo": {"DomainName": "filter-source-b"},
            "DestinationDomainInfo": {"DomainName": "filter-destination"}
        }),
    ];
    let mut ids = Vec::new();
    for request in &requests {
        let answer = create_connection_in(&router, REGION, request).await;
        assert_eq!(
            answer.status, 200,
            "creating {request} must succeed, got {body}",
            body = answer.body
        );
        ids.push(answer.text("filter fixture", "/CrossClusterSearchConnectionId"));
    }

    // The whole set, in the order the store reports it: the order every paged walk
    // must reproduce.
    let all = describe_connections_in(&router, REGION, &json!({})).await;
    let order = reported_ids(&all);
    assert_eq!(
        order.len(),
        3,
        "every created connection must be reported, got {body}",
        body = all.body
    );

    // Filter names and their sources: the five the `Filters` member documents for
    // this operation, plus a name it documents for none.
    let account = engine::emulator_account_id().to_string();
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the connection's own id",
            json!([{"Name": "cross-cluster-search-connection-id", "Values": [ids[1]]}]),
            vec![ids[1].clone()],
        ),
        (
            "source domain name",
            json!([{"Name": "source-domain-info.domain-name", "Values": ["filter-source-b"]}]),
            vec![ids[2].clone()],
        ),
        (
            "two values of one filter are alternatives",
            json!([{
                "Name": "source-domain-info.domain-name",
                "Values": ["filter-source-a", "filter-source-b"]
            }]),
            order.clone(),
        ),
        (
            "destination domain name selects every connection to it",
            json!([{"Name": "destination-domain-info.domain-name", "Values": ["filter-destination"]}]),
            order.clone(),
        ),
        (
            "destination owner id",
            json!([{"Name": "destination-domain-info.owner-id", "Values": ["210987654321"]}]),
            vec![ids[1].clone()],
        ),
        (
            "destination owner id of this account",
            json!([{"Name": "destination-domain-info.owner-id", "Values": [account]}]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "destination region",
            json!([{"Name": "destination-domain-info.region", "Values": [OTHER_REGION]}]),
            vec![ids[1].clone()],
        ),
        (
            "two filters are cumulative",
            json!([
                {"Name": "source-domain-info.domain-name", "Values": ["filter-source-a"]},
                {"Name": "destination-domain-info.region", "Values": [OTHER_REGION]}
            ]),
            vec![ids[1].clone()],
        ),
        (
            "two filters that no one connection satisfies",
            json!([
                {"Name": "source-domain-info.domain-name", "Values": ["filter-source-b"]},
                {"Name": "destination-domain-info.region", "Values": [OTHER_REGION]}
            ]),
            Vec::new(),
        ),
        (
            "a value no connection carries",
            json!([{"Name": "source-domain-info.domain-name", "Values": ["filter-source-z"]}]),
            Vec::new(),
        ),
        (
            "a filter name this operation documents for no property",
            json!([{"Name": "connection-alias", "Values": ["from-a-local"]}]),
            Vec::new(),
        ),
    ];
    for (case, filters, expected) in cases {
        let answer = describe_connections_in(&router, REGION, &json!({"Filters": filters})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: describe must succeed, got {body}",
            body = answer.body
        );
        let mut selected = reported_ids(&answer);
        let mut expected_sorted = expected.clone();
        selected.sort();
        expected_sorted.sort();
        assert_eq!(
            selected, expected_sorted,
            "case {case:?}: expected {expected_sorted:?}, got {selected:?}"
        );
    }

    // Pagination: every page size from 1 to past the set walks the whole set once,
    // in the same order, and the last page carries no token.
    for page_size in 1..=4 {
        let case = format!("MaxResults = {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let answer = describe_connections_in(&router, REGION, &body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: describe must succeed, got {body}",
                body = answer.body
            );
            let page = reported_ids(&answer);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} connections, got {page:?}"
            );
            walked.extend(page);
            token = answer
                .body
                .pointer("/NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, order,
            "case {case:?}: a paged walk must report the whole set in order, got {walked:?}"
        );
    }

    // A token naming no place in the set is the modelled refusal.
    for case in ["a token this service never minted", &ids[0].replace("ccs-", "xx-")] {
        let answer =
            describe_connections_in(&router, REGION, &json!({"NextToken": case})).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unknown token must be refused, got {status} {body}",
            status = answer.status,
            body = answer.body
        );
        assert!(
            answer
                .error_type
                .contains("InvalidPaginationTokenException"),
            "case {case:?}: expected InvalidPaginationTokenException, got {actual:?}",
            actual = answer.error_type
        );
    }

    // A token that names a connection the filters exclude names no place in THAT set.
    let answer = describe_connections_in(
        &router,
        REGION,
        &json!({
            "NextToken": ids[1],
            "Filters": [{"Name": "source-domain-info.domain-name", "Values": ["filter-source-b"]}]
        }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"token outside the filtered set\": must be refused, got {status} {body}",
        status = answer.status,
        body = answer.body
    );
}

/// Rule 6: a connection belongs to the Region its create was signed for.
#[tokio::test]
async fn a_connection_belongs_to_the_region_it_was_created_in() {
    let router = router();
    create_domain_in(&router, REGION, "region-source").await;
    create_domain_in(&router, REGION, "region-destination").await;

    let created = create_connection_in(
        &router,
        REGION,
        &json!({
            "ConnectionAlias": "regional",
            "SourceDomainInfo": {"DomainName": "region-source"},
            "DestinationDomainInfo": {"DomainName": "region-destination"}
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"create in {REGION}\": must succeed, got {body}",
        body = created.body
    );
    let id = created.text("regional", "/CrossClusterSearchConnectionId");

    for (case, region, expected) in [
        ("the Region it was created in", REGION, true),
        ("another Region", OTHER_REGION, false),
    ] {
        let answer = describe_connections_in(&router, region, &json!({})).await;
        assert_eq!(
            connection_with(&answer, &id).is_some(),
            expected,
            "case {case:?}: expected reported = {expected}, got {body}",
            body = answer.body
        );
    }
}

/// Rule 8: the documented per-source-domain limit on cross-cluster search
/// connections, which is per SOURCE DOMAIN and not per account.
#[tokio::test]
async fn a_source_domain_holds_at_most_the_documented_number_of_connections() {
    // `crate::state::MAX_CONNECTIONS_PER_DOMAIN` — the service's documented limit on
    // the cross-cluster search connections one domain takes part in.
    const LIMIT: usize = 20;

    let router = router();
    for domain in ["quota-source", "quota-source-other", "quota-destination"] {
        create_domain_in(&router, REGION, domain).await;
    }

    let connection = |source: &str, alias: String| {
        json!({
            "ConnectionAlias": alias,
            "SourceDomainInfo": {"DomainName": source},
            "DestinationDomainInfo": {"DomainName": "quota-destination"}
        })
    };

    for nth in 0..LIMIT {
        let answer =
            create_connection_in(&router, REGION, &connection("quota-source", format!("a{nth}")))
                .await;
        assert_eq!(
            answer.status, 200,
            "case \"connection {nth} of {LIMIT}\": must be accepted, got {status} {body}",
            status = answer.status,
            body = answer.body
        );
    }

    let refused = create_connection_in(
        &router,
        REGION,
        &connection("quota-source", "one-too-many".to_string()),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case \"past the per-domain limit\": expected HTTP 409, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert!(
        refused.error_type.contains("LimitExceededException"),
        "case \"past the per-domain limit\": expected LimitExceededException, got {actual:?}",
        actual = refused.error_type
    );
    let message = refused.text("past the per-domain limit", "/message");
    for expected in ["quota-source", &LIMIT.to_string()] {
        assert!(
            message.contains(expected),
            "case \"past the per-domain limit\": the refusal must name {expected:?}, got \
             {message:?}"
        );
    }

    // The bound is per source domain: another domain still takes a connection.
    let other = create_connection_in(
        &router,
        REGION,
        &connection("quota-source-other", "first".to_string()),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "case \"another source domain at its own allowance\": must be accepted, got {status} \
         {body}",
        status = other.status,
        body = other.body
    );

    // The refused create stored nothing: the limit's worth plus the other domain's.
    let describe = describe_connections_in(&router, REGION, &json!({"MaxResults": 100})).await;
    assert_eq!(
        reported_ids(&describe).len(),
        LIMIT + 1,
        "a refused create must store nothing, got {body}",
        body = describe.body
    );
}

/// The describe matches a BOUNDED amount of filter work: a request at either cap is
/// answered, one past it is refused rather than scanned.
#[tokio::test]
async fn a_describe_asking_for_more_filter_work_than_the_cap_is_refused() {
    // The caps `describe_outbound_cross_cluster_search_connections.rs` enforces.
    const MAX_FILTERS: usize = 10;
    const MAX_VALUES_PER_FILTER: usize = 1_000;

    let router = router();
    create_domain_in(&router, REGION, "capped-source").await;
    create_domain_in(&router, REGION, "capped-destination").await;
    let created = create_connection_in(
        &router,
        REGION,
        &json!({
            "ConnectionAlias": "capped",
            "SourceDomainInfo": {"DomainName": "capped-source"},
            "DestinationDomainInfo": {"DomainName": "capped-destination"}
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture connection must be created, got {body}",
        body = created.body
    );

    /// `count` filters, each naming the source domain the fixture connection has.
    fn filters(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|_| {
                    json!({
                        "Name": "source-domain-info.domain-name",
                        "Values": ["capped-source"]
                    })
                })
                .collect(),
        )
    }

    /// One filter carrying `count` values, the last of which matches.
    fn values(count: usize) -> Value {
        let mut values: Vec<Value> = (1..count).map(|nth| json!(format!("no-match-{nth}"))).collect();
        values.push(json!("capped-source"));
        json!([{"Name": "source-domain-info.domain-name", "Values": values}])
    }

    let cases: Vec<(&str, Value, u16, usize)> = vec![
        ("one filter", filters(1), 200, 1),
        ("filters exactly at the cap", filters(MAX_FILTERS), 200, 1),
        ("one filter past the cap", filters(MAX_FILTERS + 1), 400, 0),
        (
            "values exactly at the cap",
            values(MAX_VALUES_PER_FILTER),
            200,
            1,
        ),
        (
            "one value past the cap",
            values(MAX_VALUES_PER_FILTER + 1),
            400,
            0,
        ),
    ];
    for (case, filters, status, reported) in cases {
        let answer = describe_connections_in(&router, REGION, &json!({"Filters": filters})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {actual} {body}",
            actual = answer.status,
            body = answer.body
        );
        if status == 200 {
            assert_eq!(
                reported_ids(&answer).len(),
                reported,
                "case {case:?}: expected {reported} connection(s), got {body}",
                body = answer.body
            );
        } else {
            assert!(
                answer.error_type.contains("ValidationException"),
                "case {case:?}: expected ValidationException, got {actual:?}",
                actual = answer.error_type
            );
            let message = answer.text(case, "/message");
            assert!(
                message.contains("Filters"),
                "case {case:?}: the refusal must name the member, got {message:?}"
            );
        }
    }
}
