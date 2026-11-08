// SPDX-License-Identifier: CC0-1.0

//! What a connection READ reports about the connection, over the wire.
//!
//! THE RULE: every reader of a connection renders the whole stored row — including the
//! connection's ownership, which for a connection created in the calling account is
//! owned by that account — and the connection's provider type is the provider the
//! connection talks to: the one the create stated, or, when the create named only a
//! host, the provider that host is installed with (a host's provider type is modelled
//! required), persisted on the connection so every reader and every resource derived
//! from the connection reports the same provider. A connection whose create named
//! neither states no provider, so the member is absent.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over: which provider the connection talks to (every value the modelled
//! `ProviderType` enum admits), and WHICH WAY the create names it — stated in the
//! request, taken from the host, stated by both, or named by neither. The recorded
//! units' cases (a connection created with a provider type, and one created with only a
//! GitHub Enterprise Server host) are two rows of that table. Each row is read back
//! through both readers — `GetConnection` and `ListConnections` — because each
//! serializes the projection itself.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a
//! signed SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{PROVIDER_TYPES, call, router};
use super::host_association::create_plain_host;

/// The ownership every connection this emulator mints reports: it is created in the
/// calling account, which is therefore the account that owns it.
const OWNED: &str = "Owned";

/// One row of the table: how a create names the provider the connection talks to.
struct ProviderCase {
    /// What a failing assertion names this row by.
    label: String,
    /// The `ProviderType` the request states, or `None` for a request that omits it.
    stated: Option<&'static str>,
    /// The provider the host named by the request is installed with, or `None` for a
    /// request that names no host.
    host_provider: Option<&'static str>,
    /// The `ProviderType` the connection must report, or `None` when the connection
    /// names no provider at all.
    expected: Option<&'static str>,
}

/// The table: for each value the modelled `ProviderType` admits, the three ways a
/// create can name it (stated, taken from the host, stated by both), plus the one row
/// in which nothing names a provider.
///
/// The provider catalog is `connection_lifecycle::PROVIDER_TYPES`, which is the
/// enum's own `values` list from
/// `smithy_query(query="shape", shape="ProviderType")` — so a value added to the model
/// joins this table rather than being left untested.
fn provider_cases() -> Vec<ProviderCase> {
    let mut cases = Vec::new();
    for provider in PROVIDER_TYPES {
        cases.push(ProviderCase {
            label: format!("the request states {provider}"),
            stated: Some(provider),
            host_provider: None,
            expected: Some(provider),
        });
        cases.push(ProviderCase {
            label: format!("the request names only a host installed with {provider}"),
            stated: None,
            host_provider: Some(provider),
            expected: Some(provider),
        });
        cases.push(ProviderCase {
            label: format!("the request states {provider} and names a host installed with it"),
            stated: Some(provider),
            host_provider: Some(provider),
            expected: Some(provider),
        });
    }
    // The one row nothing names a provider on: the modelled `CreateConnection` input makes both
    // `ProviderType` and `HostArn` optional (`smithy_query(query="operation",
    // operation="CreateConnection")` lists `ConnectionName` as its only required member), so a
    // create naming neither is accepted and the connection it mints names no provider — there is
    // no host to take one from. The row is here for the derivation rule's boundary, not as a claim
    // about that create's refusal.
    cases.push(ProviderCase {
        label: "the request names neither a provider type nor a host".to_string(),
        stated: None,
        host_provider: None,
        expected: None,
    });
    cases
}

/// Create the connection one row describes and return `(ConnectionArn, HostArn)`.
async fn create_connection(
    router: &MultiServiceRouter,
    case: &ProviderCase,
    index: usize,
) -> (String, Option<String>) {
    let host_arn = match case.host_provider {
        Some(provider) => {
            Some(create_plain_host(router, &format!("host-{index}"), provider).await)
        }
        None => None,
    };

    let mut body = serde_json::json!({ "ConnectionName": format!("conn-{index}") });
    if let Some(stated) = case.stated {
        body["ProviderType"] = serde_json::json!(stated);
    }
    if let Some(host_arn) = host_arn.as_deref() {
        body["HostArn"] = serde_json::json!(host_arn);
    }

    let (status, created) = call(router, "CreateConnection", body).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from CreateConnection, got {status} {created}",
        case.label
    );
    let connection_arn = created["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no ConnectionArn: {created}", case.label))
        .to_string();
    (connection_arn, host_arn)
}

/// The `Connection` structure every reader of one connection must report for a row.
fn expected_connection(
    case: &ProviderCase,
    index: usize,
    connection_arn: &str,
    host_arn: Option<&str>,
) -> serde_json::Value {
    let mut expected = serde_json::json!({
        "ConnectionName": format!("conn-{index}"),
        "ConnectionArn": connection_arn,
        "OwnerAccountId": engine::emulator_account_id(),
        "ConnectionStatus": "PENDING",
        "OwnershipType": OWNED,
    });
    if let Some(provider) = case.expected {
        expected["ProviderType"] = serde_json::json!(provider);
    }
    if let Some(host_arn) = host_arn {
        expected["HostArn"] = serde_json::json!(host_arn);
    }
    expected
}

/// `GetConnection` reports the ownership and the provider type of every connection,
/// however the create named that provider — and a second read reports the same.
#[tokio::test]
async fn get_connection_reports_the_ownership_and_the_resolved_provider_type() {
    let router = router();

    for (index, case) in provider_cases().iter().enumerate() {
        let (connection_arn, host_arn) = create_connection(&router, case, index).await;

        let (status, got) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetConnection, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["Connection"],
            expected_connection(case, index, &connection_arn, host_arn.as_deref()),
            "case {:?}: expected the whole stored connection, got {got}",
            case.label
        );

        // A read does not change what the next caller sees.
        let (_, again) = call(
            &router,
            "GetConnection",
            serde_json::json!({ "ConnectionArn": connection_arn }),
        )
        .await;
        assert_eq!(
            again, got,
            "case {:?}: a second read must report the same connection, got {again}",
            case.label
        );
    }
}

/// `ListConnections` renders each element from the same projection, so every listed
/// element equals what `GetConnection` reports for that connection — ownership and
/// resolved provider type included.
#[tokio::test]
async fn listed_connections_carry_the_same_ownership_and_provider_type() {
    let router = router();

    let cases = provider_cases();
    let mut expected_elements = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let (connection_arn, host_arn) = create_connection(&router, case, index).await;
        expected_elements.push((
            case,
            expected_connection(case, index, &connection_arn, host_arn.as_deref()),
        ));
    }

    // One page per row, so the listing is walked through its own token rather than in a
    // single response: every element the report renders is checked.
    let mut listed: Vec<serde_json::Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = serde_json::json!({ "MaxResults": 1 });
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = serde_json::json!(token);
        }
        let (status, page) = call(&router, "ListConnections", body).await;
        assert_eq!(
            status, 200,
            "ListConnections: expected 200, got {status} {page}"
        );
        let entries = page["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("ListConnections reported no Connections: {page}"));
        listed.extend(entries.iter().cloned());
        match page["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }

    assert_eq!(
        listed.len(),
        expected_elements.len(),
        "expected every created connection to be listed, got {listed:?}"
    );
    for (case, expected) in &expected_elements {
        assert!(
            listed.contains(expected),
            "case {:?}: expected the listed element {expected}, got {listed:?}",
            case.label
        );
    }
}

/// The provider type a connection took from its host is PERSISTED on the connection, so
/// everything that keys on a connection's provider sees it: `ListConnections`'
/// `ProviderTypeFilter` matches the connection, and a repository link created against it
/// reports that provider as its own.
#[tokio::test]
async fn a_host_derived_provider_type_drives_the_filter_and_the_derived_link() {
    let router = router();

    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        let case = ProviderCase {
            label: format!("a connection on a host installed with {provider}"),
            stated: None,
            host_provider: Some(provider),
            expected: Some(provider),
        };
        let (connection_arn, host_arn) = create_connection(&router, &case, index).await;

        let (status, filtered) = call(
            &router,
            "ListConnections",
            serde_json::json!({ "ProviderTypeFilter": provider }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListConnections, got {status} {filtered}",
            case.label
        );
        let arns: Vec<&str> = filtered["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no Connections: {filtered}", case.label))
            .iter()
            .filter_map(|entry| entry["ConnectionArn"].as_str())
            .collect();
        assert!(
            arns.contains(&connection_arn.as_str()),
            "case {:?}: expected the filter on the host's provider to match the connection, got {arns:?}",
            case.label
        );

        // `RepositoryLinkInfo.ProviderType` is the provider type of the connection the
        // link is associated with, so a link on a host-based connection reports the
        // host's provider too.
        let (status, link) = call(
            &router,
            "CreateRepositoryLink",
            serde_json::json!({
                "ConnectionArn": connection_arn,
                "OwnerId": format!("owner-{index}"),
                "RepositoryName": format!("repo-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateRepositoryLink, got {status} {link}",
            case.label
        );
        assert_eq!(
            link["RepositoryLinkInfo"]["ProviderType"],
            serde_json::json!(provider),
            "case {:?}: expected the link to report the connection's provider, got {link}",
            case.label
        );

        // The host the provider came from is the one the connection reports.
        assert_eq!(
            host_arn.as_deref(),
            Some(
                call(
                    &router,
                    "GetConnection",
                    serde_json::json!({ "ConnectionArn": connection_arn }),
                )
                .await
                .1["Connection"]["HostArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {:?}: no HostArn reported", case.label))
            ),
            "case {:?}: expected the connection to report the host it was created on",
            case.label
        );
    }
}
