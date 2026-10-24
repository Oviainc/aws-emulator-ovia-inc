// SPDX-License-Identifier: CC0-1.0

//! How this service answers a caller-supplied reference to a HOST, across the readers of
//! that reference.
//!
//! The RULE: an ARN a request supplies naming another resource of this service is
//! RESOLVED against that resource's store before the request is served — a reference that
//! resolves is used, and an in-account one naming no such resource is refused with the
//! `ResourceNotFoundException` the model declares for the operation, naming the ARN that
//! was supplied. It is never stored as a dangling pointer and never silently reduced to
//! an empty result.
//!
//! `host_arn_rules.rs` pins that rule on the two members that take a host reference
//! (`CreateConnection.HostArn` and `ListConnections.HostArnFilter`,
//! `smithy_query(query="operation",operation="CreateConnection,ListConnections")`) and the
//! authorization that precedes it. What this suite adds is the two things a single call
//! cannot show:
//!
//! * a refusal is told apart from an empty result — a host that EXISTS and carries no
//!   connection answers a 200 empty page, where one that does not exist is refused; and
//! * resolving a reference is a READ, so a refused request leaves both stores as they
//!   were and a later caller sees no trace of it (driven over repeated rounds).
//!
//! Hosts are created through `CreateHost`, so what a connection names or a filter selects
//! is a host this service actually minted. Every call goes over the wire through the real
//! `MultiServiceRouter`, reusing the `connection_lifecycle` suite's harness.

use serde_json::{json, Value};

use super::connection_lifecycle::{
    error_code, error_message, Emulator, ACCOUNT_ID, HOST_PROVIDER_TYPE, REGION,
};

/// Host ARNs that satisfy `HostArn`'s modeled pattern
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$` and name no host this
/// emulator holds — a plain name, a UUID, another region and another partition, so the
/// refusal is not keyed to one shape of ARN.
///
/// Every row carries the CALLER'S account, which is what makes the answer an absence: a
/// host ARN in another account is refused as an authorization failure before any store is
/// consulted, which is `host_arn_rules.rs`'s subject.
fn absent_host_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a plain host name",
            format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/never-created"),
        ),
        (
            "a uuid host id",
            format!(
                "arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "another region",
            format!("arn:aws:codestar-connections:eu-west-1:{ACCOUNT_ID}:host/elsewhere"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:codestar-connections:cn-north-1:{ACCOUNT_ID}:host/elsewhere"),
        ),
    ]
}

/// Assert `response` is the modelled absence and that it names `arn`.
///
/// `ResourceNotFoundException` is modeled `@httpError(404)`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), but this service
/// answers an absent resource at the status its recorded wire does — see
/// `state::RESOURCE_NOT_FOUND_STATUS`. The host wording names the ARN it was handed.
fn assert_host_not_found(case: &str, status: u16, response: &Value, arn: &str) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: expected the modelled absence for {arn}, got {status} {response}"
    );
    let message = error_message(response);
    assert!(
        message.contains(arn),
        "case {case:?}: the refusal must name the ARN that was supplied ({arn}), got \
         {message:?}"
    );
}

/// The connection ARNs `ListConnections` reports for `filter`, asserting it answered 200.
async fn listed_with_filter(emulator: &Emulator, case: &str, filter: &str) -> Vec<String> {
    let (status, response) = emulator
        .call("ListConnections", json!({ "HostArnFilter": filter }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListConnections to answer 200, got {status} {response}"
    );
    response["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Connections in {response}"))
        .iter()
        .map(|connection| {
            connection["ConnectionArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: a host that EXISTS and carries no connection is an empty page, while one that
/// does not exist is refused — "this host is not there" and "this host has no
/// connections" are different answers.
#[tokio::test]
async fn an_absent_host_is_refused_where_a_host_without_connections_is_an_empty_page() {
    let emulator = Emulator::new();

    let used = emulator.create_host("used").await;
    let unused = emulator.create_host("unused").await;

    let hosted = emulator
        .create(
            "hosted",
            json!({
                "ConnectionName": "hosted",
                "ProviderType": HOST_PROVIDER_TYPE,
                "HostArn": used,
            }),
        )
        .await;
    emulator
        .create(
            "unhosted",
            json!({ "ConnectionName": "unhosted", "ProviderType": "GitHub" }),
        )
        .await;

    for (case, arn) in absent_host_arns() {
        let (status, response) = emulator
            .call("ListConnections", json!({ "HostArnFilter": arn }))
            .await;
        assert_host_not_found(case, status, &response, &arn);
    }

    // (case, filter value, the connections it selects)
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "a host with one connection",
            used.as_str(),
            vec![hosted.clone()],
        ),
        ("a host with none", unused.as_str(), vec![]),
    ];
    for (case, filter, expected) in rows {
        let got = listed_with_filter(&emulator, case, filter).await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// RULE: resolving a host reference is a READ — a refused request leaves the host store
/// and the connection store exactly as they were, so a later caller sees no trace of it.
///
/// Repeated rounds, because a refusal that consumed something (a sequence number, a quota,
/// a record) would only show on the call after it.
#[tokio::test]
async fn a_refused_host_reference_changes_nothing() {
    let emulator = Emulator::new();
    let host = emulator.create_host("stable").await;
    let connection = emulator
        .create(
            "seed",
            json!({
                "ConnectionName": "seed",
                "ProviderType": HOST_PROVIDER_TYPE,
                "HostArn": host,
            }),
        )
        .await;

    let absent = format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:host/never-created");
    for round in 0..3 {
        let case = format!("round-{round}");
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({
                    "ConnectionName": "rejected",
                    "ProviderType": HOST_PROVIDER_TYPE,
                    "HostArn": absent,
                }),
            )
            .await;
        assert_host_not_found(&case, status, &response, &absent);

        let (status, response) = emulator
            .call("ListConnections", json!({ "HostArnFilter": absent }))
            .await;
        assert_host_not_found(&case, status, &response, &absent);

        // The host that does exist still resolves, and still selects its one connection.
        let got = listed_with_filter(&emulator, &case, &host).await;
        assert_eq!(
            got,
            vec![connection.clone()],
            "case {case:?}: the refused requests changed what the filter selects"
        );

        // And the whole report is still the two connections the creates stored: the
        // refused creates minted nothing.
        let (status, listed) = emulator.call("ListConnections", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListConnections to answer 200, got {status} {listed}"
        );
        let count = listed["Connections"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, 1,
            "case {case:?}: expected a refused create to store nothing, got {count} \
             connections: {listed}"
        );
    }
}
