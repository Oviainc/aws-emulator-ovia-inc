// SPDX-License-Identifier: CC0-1.0

//! `CreateConnection`'s input rules, and how every caller-supplied host ARN is read.
//!
//! Two rules, each pinned over a TABLE rather than over one recorded request:
//!
//! * a connection names the third-party provider it reaches either DIRECTLY
//!   (`ProviderType`) or through the host that holds it (`HostArn`). Both members are
//!   optional in the model, which cannot express "at least one of", so the service
//!   enforces it: a request supplying neither is refused `400 ValidationException` and
//!   creates nothing, while a request supplying either one — or both — is accepted.
//! * a host ARN a request supplies, as the host a connection is associated with
//!   (`CreateConnection.HostArn`) or as the host a report is filtered by
//!   (`ListConnections.HostArnFilter`), is AUTHORIZED against the caller's account before
//!   it is resolved against any store: an ARN whose account field is not the caller's is
//!   refused `AccessDeniedException` (403) naming the caller's account and that ARN, an
//!   in-account ARN naming no host is the modelled absence `ResourceNotFoundException`,
//!   and only an ARN naming a host the service holds is used — the connection it creates
//!   reaching its provider through that host when the request named none.
//!
//! Hosts are created through `CreateHost`, so the host a connection names or a filter
//! selects is one this service actually minted.
//!
//! Every call goes over the wire through the real `MultiServiceRouter`, reusing the
//! `connection_lifecycle` suite's harness; the target-prefix forms a client may write are
//! the `sdk_target_dispatch` suite's subject.

use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use super::connection_lifecycle::{ACCOUNT_ID, Emulator, HOST_PROVIDER_TYPE, REGION, error_code};

/// Every value the model's `ProviderType` enum accepts.
///
/// Referenced, not copied: the generated catalog
/// (`src/ops/types/shared_types.rs::PROVIDER_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="ProviderType")`), so the tables below widen with
/// the model instead of drifting from it.
const PROVIDER_TYPES: &[&str] = codestar::ops::types::shared_types::PROVIDER_TYPE_VALUES;

/// Every connection ARN the service reports.
async fn listed_arns(emulator: &Emulator) -> Vec<String> {
    let (status, body) = emulator.call("ListConnections", json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListConnections to answer 200, got {status} {body}"
    );
    body["Connections"]
        .as_array()
        .unwrap_or_else(|| panic!("no Connections list in {body}"))
        .iter()
        .filter_map(|connection| connection["ConnectionArn"].as_str())
        .map(str::to_string)
        .collect()
}

/// The message an error envelope carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response["message"]
        .as_str()
        .or_else(|| response["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A well-shaped `HostArn` (modeled `len 0..256`, pattern
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`) in `account`.
fn host_arn_in(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:codestar-connections:{region}:{account}:host/{id}")
}

/// Host ARNs that name an account OTHER than the caller's, each `(case, arn)`.
///
/// The rows differ along everything an ARN can differ in while still naming a foreign
/// account: which account, which region, and which partition — the authorization answer
/// is about the ACCOUNT field alone.
fn foreign_account_host_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "another account, same region",
            host_arn_in("aws", REGION, "111122223333", "11111111-2222-3333-4444-555555555555"),
        ),
        (
            "another account, another region",
            host_arn_in("aws", "eu-central-1", "444455556666", "enterprise"),
        ),
        (
            "another account, another partition",
            host_arn_in("aws-us-gov", REGION, "999988887777", "enterprise"),
        ),
        (
            // One digit off the caller's own account is still another account.
            "an account one digit from the caller's",
            host_arn_in("aws", REGION, "000000000001", "enterprise"),
        ),
    ]
}

/// RULE: `CreateConnection` requires at least one of `ProviderType` and `HostArn` — a
/// connection names its provider directly or through its host — so a request supplying
/// neither is refused `400 ValidationException` and creates nothing, and one supplying
/// either (or both) is accepted.
#[tokio::test]
async fn a_connection_must_name_its_provider_directly_or_through_a_host() {
    let emulator = Emulator::new();
    let host = emulator.create_host("provider-holding-host").await;

    // (case, body, accepted)
    let mut rows: Vec<(String, Value, bool)> = vec![
        (
            "neither ProviderType nor HostArn".to_string(),
            json!({ "ConnectionName": "no-provider" }),
            false,
        ),
        (
            "HostArn alone".to_string(),
            json!({ "ConnectionName": "host-backed", "HostArn": host }),
            true,
        ),
        (
            "ProviderType and HostArn together".to_string(),
            json!({
                "ConnectionName": "host-backed-typed",
                "ProviderType": "GitHubEnterpriseServer",
                "HostArn": host,
            }),
            true,
        ),
        (
            // The refusal is about the two provider members, so supplying every OTHER
            // optional member does not make the request nameable.
            "neither member, with tags supplied".to_string(),
            json!({
                "ConnectionName": "no-provider-tagged",
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
            false,
        ),
    ];
    // Every modelled provider, supplied alone, is a direct connection.
    for provider in PROVIDER_TYPES {
        rows.push((
            format!("ProviderType {provider} alone"),
            json!({ "ConnectionName": format!("direct-{provider}"), "ProviderType": provider }),
            true,
        ));
    }
    assert!(
        rows.len() >= 9,
        "the provider catalog must drive this table, got {} rows",
        rows.len()
    );

    let mut expected_arns: Vec<String> = Vec::new();
    for (case, body, accepted) in rows {
        let (status, response) = emulator.call("CreateConnection", body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateConnection to answer 200, got {status} \
                 {response}"
            );
            let arn = response["ConnectionArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectionArn in {response}"))
                .to_string();
            expected_arns.push(arn);
        } else {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the request refused as invalid, got {status} \
                 {response}"
            );
            // The caller has to fix the request from the message alone, so it names both
            // members it may supply.
            let message = error_message(&response);
            assert!(
                message.contains("ProviderType") && message.contains("HostArn"),
                "case {case:?}: the refusal must name both members, got {message:?}"
            );
        }
    }

    // A refused request mints nothing: exactly the accepted rows are addressable.
    let mut listed = listed_arns(&emulator).await;
    listed.sort_unstable();
    expected_arns.sort_unstable();
    assert_eq!(
        listed, expected_arns,
        "only the accepted rows may have created a connection"
    );
}

/// RULE: a host ARN naming another account is refused `AccessDeniedException` (403) BEFORE
/// any resource lookup, wherever a request supplies one — as `CreateConnection.HostArn`
/// or as `ListConnections.HostArnFilter` — and nothing is created.
///
/// "Before any lookup" is what the second half of each row pins: the same foreign ARN is
/// denied whether or not a host of that id exists in the caller's own account.
#[tokio::test]
async fn a_host_arn_in_another_account_is_denied_before_any_lookup() {
    let emulator = Emulator::new();
    // A host the caller DOES own, so the denials below are about the account field and
    // not about an empty store.
    let owned = emulator.create_host("owned-host").await;

    for (case, arn) in foreign_account_host_arns() {
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({ "ConnectionName": "denied", "ProviderType": "GitHubEnterpriseServer", "HostArn": arn }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected CreateConnection to deny {arn:?}, got {status} {response}"
        );
        // The message is built from the request: the caller's account and the ARN it was
        // denied, so a client can tell which of the two is wrong.
        assert_eq!(
            error_message(&response),
            format!("Account {ACCOUNT_ID} does not have access to host {arn}"),
            "case {case:?}: wrong refusal message in {response}"
        );

        let (status, response) = emulator
            .call("ListConnections", json!({ "HostArnFilter": arn }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected ListConnections to deny the filter {arn:?}, got {status} \
             {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Account {ACCOUNT_ID} does not have access to host {arn}"),
            "case {case:?}: wrong refusal message in {response}"
        );
    }

    // Nothing was created, and the host the caller owns is untouched and still usable.
    assert!(
        listed_arns(&emulator).await.is_empty(),
        "a denied CreateConnection must create no connection"
    );
    let (status, response) = emulator
        .call(
            "CreateConnection",
            json!({ "ConnectionName": "allowed", "HostArn": owned }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a host in the caller's own account must still be usable, got {status} {response}"
    );
}

/// RULE: a host ARN in the caller's OWN account that resolves to no host this service
/// holds is the modelled absence — `ResourceNotFoundException`, naming the ARN supplied —
/// for both the `CreateConnection` association and the `ListConnections` filter, and
/// nothing is created; an ARN naming a host the service holds is used.
#[tokio::test]
async fn an_in_account_host_arn_is_resolved_against_the_host_store() {
    let emulator = Emulator::new();
    let held = emulator.create_host("held-host").await;

    // Well-shaped in-account `HostArn`s that resolve to nothing: an id nothing seeded,
    // the all-zero id, a deeper resource path, and the held host's id in another region
    // (the ARN is the key, so a different region names a different host).
    let absent: Vec<(&str, String)> = vec![
        (
            "an id nothing seeded",
            host_arn_in("aws", REGION, ACCOUNT_ID, "no-such-host"),
        ),
        (
            "the all-zero host id",
            host_arn_in(
                "aws",
                REGION,
                ACCOUNT_ID,
                "00000000-0000-0000-0000-000000000000",
            ),
        ),
        (
            "a deeper resource path",
            host_arn_in("aws", REGION, ACCOUNT_ID, "held-host/extra"),
        ),
        (
            "the held host's id in another region",
            host_arn_in("aws", "eu-central-1", ACCOUNT_ID, "held-host"),
        ),
    ];

    for (case, arn) in &absent {
        let (status, response) = emulator
            .call(
                "CreateConnection",
                json!({ "ConnectionName": "absent-host", "HostArn": arn }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected CreateConnection to refuse {arn:?} as absent, got \
             {status} {response}"
        );
        assert!(
            error_message(&response).contains(arn.as_str()),
            "case {case:?}: the refusal must name the ARN supplied, got {response}"
        );

        let (status, response) = emulator
            .call("ListConnections", json!({ "HostArnFilter": arn }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected ListConnections to refuse the filter {arn:?} as absent, \
             got {status} {response}"
        );
    }

    assert!(
        listed_arns(&emulator).await.is_empty(),
        "a CreateConnection naming no host must create no connection"
    );

    // The host the store DOES hold is associated, persisted and reported back — the
    // round trip the refusals above are the other half of. No `ProviderType` is supplied,
    // so the connection reaches its provider through the host.
    let (status, response) = emulator
        .call(
            "CreateConnection",
            json!({ "ConnectionName": "held", "HostArn": held }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected CreateConnection to accept the held host, got {status} {response}"
    );
    let arn = response["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn in {response}"))
        .to_string();

    let (status, response) = emulator
        .call("GetConnection", json!({ "ConnectionArn": &arn }))
        .await;
    assert_eq!(
        (
            status,
            response["Connection"]["HostArn"].as_str(),
            response["Connection"]["ProviderType"].as_str(),
        ),
        (200, Some(held.as_str()), Some(HOST_PROVIDER_TYPE)),
        "expected GetConnection to report the host the connection was created with and \
         that host's provider, got {status} {response}"
    );

    let (status, response) = emulator
        .call("ListConnections", json!({ "HostArnFilter": &held }))
        .await;
    assert_eq!(
        (
            status,
            response["Connections"]
                .as_array()
                .map(|page| page.len())
                .unwrap_or_default(),
        ),
        (200, 1),
        "expected the filter to select the one connection on the held host, got {status} \
         {response}"
    );
    assert_eq!(
        response["Connections"][0]["ConnectionArn"].as_str(),
        Some(arn.as_str()),
        "expected the filter to select {arn:?}, got {response}"
    );
}
