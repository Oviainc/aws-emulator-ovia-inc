// SPDX-License-Identifier: CC0-1.0

//! The connector slice of AWS Config, end to end over the wire.
//!
//! Four rules are pinned here, each over a TABLE of inputs rather than one recorded
//! case:
//!
//! 1. `ListConnectors` reports exactly the connectors in state, each with the
//!    attributes the service derived at creation time (the ARN, the derived name, the
//!    provider and the tenant its configuration names), and reports nothing once a
//!    connector is deleted;
//! 2. one `ListConnectors` page carries at most `MaxResults` connectors and the
//!    `NextToken` it returns resumes the listing, so paging reports every connector
//!    exactly once at every page size;
//! 3. a filter selects the connectors whose named attribute equals one of the values
//!    the filter carries, and a filter naming anything the model does not publish is
//!    refused;
//! 4. an `Arn` member is refused as invalid when it is not a connector ARN, and
//!    answered as a missing resource when it is a connector ARN this account does not
//!    hold — the same for every operation that takes one.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// Every published `Provider` value, from
/// `smithy_query(query="shape", shape="Provider")`: `AZURE`.
const AZURE: &str = "AZURE";

/// A `Provider` value the model does NOT publish, used to check that a filter value
/// is compared rather than assumed to match.
const UNPUBLISHED_PROVIDER: &str = "GCP";

/// Every published `ConnectorFilterName` value, from
/// `smithy_query(query="shape", shape="ConnectorFilterName")`: `provider`.
const PROVIDER_FILTER: &str = "provider";

/// The `AzureTenantIdentifier` / `AzureClientIdentifier` length maximum, from
/// `smithy_query(query="shape", shape="AzureTenantIdentifier")`: 1..128.
const AZURE_IDENTIFIER_MAX_LEN: usize = 128;

/// The emulator's account id, the one every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

/// A `ConnectorConfiguration` naming the Azure provider.
fn azure_configuration(tenant: &str, client: &str) -> Value {
    json!({ "azure": { "tenantIdentifier": tenant, "clientIdentifier": client } })
}

/// The connector name the service derives for an Azure tenant, and the ARN it mints
/// for it in `region`: the provider it connects to and the tenant within it.
fn expected_name_and_arn(region: &str, tenant: &str) -> (String, String) {
    let name = format!("{}-{tenant}", AZURE.to_lowercase());
    (
        name.clone(),
        format!("arn:aws:config:{region}:{ACCOUNT}:connector/{name}"),
    )
}

/// Create a connector for `tenant`, returning the ARN `PutConnector` minted.
async fn put_connector(emulator: &Emulator, tenant: &str, client: &str) -> String {
    let created = emulator
        .ok(
            "PutConnector",
            &json!({ "ConnectorConfiguration": azure_configuration(tenant, client) }),
        )
        .await;
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutConnector must answer an Arn, got {created}"))
        .to_string()
}

/// Every connector ARN `ListConnectors` reports, paged at `max_results` (0 asks for
/// no particular page size), together with the size of each page it served.
async fn list_connector_arns(emulator: &Emulator, max_results: i64) -> (Vec<String>, Vec<usize>) {
    let mut arns = Vec::new();
    let mut page_sizes = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": max_results });
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("ListConnectors", &request).await;
        let summaries = page["ConnectorSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("ListConnectors must answer ConnectorSummaries, got {page}"))
            .clone();
        page_sizes.push(summaries.len());
        for summary in &summaries {
            arns.push(
                summary["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a ConnectorSummary must carry an arn, got {summary}"))
                    .to_string(),
            );
        }
        match page["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    (arns, page_sizes)
}

/// The summaries one unpaginated `ListConnectors` call reports, with `filters`.
async fn list_with_filters(emulator: &Emulator, filters: &Value) -> Vec<Value> {
    let listed = emulator
        .ok("ListConnectors", &json!({ "Filters": filters }))
        .await;
    listed["ConnectorSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListConnectors must answer ConnectorSummaries, got {listed}"))
        .clone()
}

#[tokio::test]
async fn a_connector_is_listed_and_read_back_exactly_as_it_was_created() {
    let emulator = Emulator::new();
    // Rows differ along the form and the length of the identifiers the configuration
    // carries: the GUID form Microsoft Entra ID uses, a one-character identifier (the
    // modelled length minimum), a domain-form tenant, and the modelled length
    // maximum. None of them is the identifier the issue's recording used.
    let max_length_tenant = "t".repeat(AZURE_IDENTIFIER_MAX_LEN);
    let max_length_client = "c".repeat(AZURE_IDENTIFIER_MAX_LEN);
    let cases: [(&str, &str, &str); 4] = [
        (
            "guid identifiers",
            "72f988bf-86f1-41af-91ab-2d7cd011db47",
            "04b07795-8ddb-461a-bbee-02f9e1bf7b46",
        ),
        ("shortest identifiers", "t", "c"),
        (
            "domain-form tenant",
            "contoso.onmicrosoft.com",
            "config.connector.app",
        ),
        (
            "longest identifiers",
            max_length_tenant.as_str(),
            max_length_client.as_str(),
        ),
    ];

    for (case, tenant, client) in cases {
        let (expected_name, expected_arn) = expected_name_and_arn(&emulator.region, tenant);
        let arn = put_connector(&emulator, tenant, client).await;
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: PutConnector must mint a connector ARN naming the derived \
             connector name, got {arn:?}"
        );

        let read = emulator.ok("GetConnector", &json!({ "Arn": arn })).await;
        let connector = &read["Connector"];
        assert_eq!(
            connector["name"].as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: GetConnector must report the derived name, got {connector}"
        );
        assert_eq!(
            connector["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: GetConnector must report the ARN it was created with, got \
             {connector}"
        );
        assert_eq!(
            connector["connectorConfiguration"],
            azure_configuration(tenant, client),
            "case {case:?}: GetConnector must report the configuration it was created \
             with, got {connector}"
        );
        let created_time = connector["createdTime"].as_f64().unwrap_or_default();
        assert!(
            created_time > 0.0,
            "case {case:?}: GetConnector must report when the connector was created, \
             got {connector}"
        );

        let summaries = list_with_filters(&emulator, &json!([])).await;
        let listed = summaries
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(expected_arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListConnectors must report the created connector, got {summaries:?}")
            });
        assert_eq!(
            (
                listed["name"].as_str(),
                listed["provider"].as_str(),
                listed["tenantIdentifier"].as_str(),
                listed["createdTime"].as_f64(),
            ),
            (
                Some(expected_name.as_str()),
                Some(AZURE),
                Some(tenant),
                Some(created_time),
            ),
            "case {case:?}: a summary must report the attributes state holds for the \
             connector, got {listed}"
        );
    }

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        cases.len(),
        "every created connector must be listed exactly once, got {arns:?}"
    );
}

#[tokio::test]
async fn paging_reports_every_connector_exactly_once_at_every_page_size() {
    let emulator = Emulator::new();
    const CONNECTORS: usize = 7;
    let mut created = Vec::new();
    for index in 0..CONNECTORS {
        created.push(put_connector(&emulator, &format!("tenant-{index}"), "client").await);
    }
    created.sort();

    // `MaxResults` is 0..100 with a default of 0 ("no particular page size"), from
    // `smithy_query(query="shape", shape="ListConnectorsMaxResults")`. The rows span
    // that range: both published bounds, one past the maximum, a page smaller than
    // the store, a page the size of the store and a page larger than it.
    let cases: [(&str, i64); 6] = [
        ("unspecified page size", 0),
        ("one per page", 1),
        ("smaller than the store", 3),
        ("exactly the store", CONNECTORS as i64),
        ("larger than the store", CONNECTORS as i64 + 1),
        ("the published maximum", 100),
    ];

    for (case, max_results) in cases {
        let (mut arns, page_sizes) = list_connector_arns(&emulator, max_results).await;
        arns.sort();
        assert_eq!(
            arns, created,
            "case {case:?}: paging must report every connector exactly once, got {arns:?}"
        );
        if max_results > 0 {
            let oversized: Vec<usize> = page_sizes
                .iter()
                .copied()
                .filter(|size| *size > max_results as usize)
                .collect();
            assert!(
                oversized.is_empty(),
                "case {case:?}: no page may carry more than MaxResults connectors, got \
                 page sizes {page_sizes:?}"
            );
        }
    }

    // One past the published maximum is refused by the modelled range, not served.
    let (status, code) = emulator
        .refused("ListConnectors", &json!({ "MaxResults": 101 }))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"past the published maximum\": expected HTTP 400 ValidationException, \
         got {status} {code}"
    );

    // A token this operation did not issue is malformed, not an empty page. Rows
    // differ along HOW the value fails to be a token this operation handed out: one
    // that is not even base64, and one that is well-formed base64 of something that
    // is not a key this operation pages on.
    for (case, token) in [
        ("not base64 at all", "not a token!!"),
        ("base64 of a value that is no page key", "YWJj"),
    ] {
        let (status, code) = emulator
            .refused("ListConnectors", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_filter_selects_the_connectors_whose_attribute_it_matches() {
    let emulator = Emulator::new();
    const CONNECTORS: usize = 3;
    for index in 0..CONNECTORS {
        put_connector(&emulator, &format!("tenant-{index}"), "client").await;
    }

    // Every row is a filter list and the number of the three stored connectors it
    // selects. Each stored connector's provider is `AZURE`, the only published one.
    let cases: [(&str, Value, usize); 5] = [
        ("no filters", json!([]), CONNECTORS),
        (
            "the stored provider",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [AZURE] }]),
            CONNECTORS,
        ),
        (
            "a provider nothing was created for",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [UNPUBLISHED_PROVIDER] }]),
            0,
        ),
        (
            "either provider",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [UNPUBLISHED_PROVIDER, AZURE] }]),
            CONNECTORS,
        ),
        (
            "no value to match",
            json!([{ "filterName": PROVIDER_FILTER, "filterValues": [] }]),
            0,
        ),
    ];

    for (case, filters, expected) in cases {
        let summaries = list_with_filters(&emulator, &filters).await;
        assert_eq!(
            summaries.len(),
            expected,
            "case {case:?}: expected {expected} connector(s), got {summaries:?}"
        );
        for summary in &summaries {
            assert_eq!(
                summary["provider"].as_str(),
                Some(AZURE),
                "case {case:?}: a selected connector must satisfy the filter, got {summary}"
            );
        }
    }

    // A filter this service cannot apply is refused: answering it with the connectors
    // it would have excluded is not an answer to the request.
    let unsupported: [(&str, Value); 3] = [
        (
            "a filter name the model does not publish",
            json!([{ "filterName": "name", "filterValues": ["azure-tenant-0"] }]),
        ),
        (
            "a filter naming nothing",
            json!([{ "filterValues": [AZURE] }]),
        ),
        (
            "a supported filter beside an unsupported one",
            json!([
                { "filterName": PROVIDER_FILTER, "filterValues": [AZURE] },
                { "filterName": "tenantIdentifier", "filterValues": ["tenant-0"] },
            ]),
        ),
    ];
    for (case, filters) in unsupported {
        let (status, code) = emulator
            .refused("ListConnectors", &json!({ "Filters": filters }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_deleted_connector_is_reported_by_neither_a_read_nor_a_listing() {
    let emulator = Emulator::new();
    let kept = put_connector(&emulator, "tenant-kept", "client").await;
    let deleted = put_connector(&emulator, "tenant-deleted", "client").await;

    emulator
        .ok("DeleteConnector", &json!({ "Arn": deleted }))
        .await;

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns,
        vec![kept.clone()],
        "a listing must report the connectors still in state, got {arns:?}"
    );

    for operation in ["GetConnector", "DeleteConnector"] {
        let (status, code) = emulator
            .refused(operation, &json!({ "Arn": deleted }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation:?}: a deleted connector must be answered as missing, got \
             {status} {code}"
        );
    }

    // The kept connector is untouched by the delete and by the refusals above.
    let read = emulator
        .ok("GetConnector", &json!({ "Arn": kept }))
        .await;
    assert_eq!(
        read["Connector"]["arn"].as_str(),
        Some(kept.as_str()),
        "the connector that was not deleted must still be readable, got {read}"
    );
}

#[tokio::test]
async fn an_arn_member_is_refused_when_invalid_and_answered_as_missing_when_unknown() {
    let emulator = Emulator::new();
    put_connector(&emulator, "tenant-0", "client").await;

    // Rows differ along WHY the value is not a connector ARN of this service: not an
    // ARN at all, no resource portion, no connector name, another service's ARN, and
    // another resource type of this service.
    let invalid: [(&str, &str); 6] = [
        ("not an ARN", "tenant-0"),
        ("too few ARN segments", "arn:aws:config:us-east-1:000000000000"),
        (
            "no connector name",
            "arn:aws:config:us-east-1:000000000000:connector",
        ),
        (
            "an empty connector name",
            "arn:aws:config:us-east-1:000000000000:connector/",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:us-east-1:000000000000:connector/azure-tenant-0",
        ),
        (
            "another resource type of this service",
            "arn:aws:config:us-east-1:000000000000:configuration-recorder/default/abc",
        ),
    ];
    // A well-formed connector ARN that names no connector this account holds in this
    // Region is missing, not invalid.
    let unknown: [(&str, &str); 2] = [
        (
            "a connector this account never created",
            "arn:aws:config:us-east-1:000000000000:connector/azure-absent",
        ),
        (
            "a connector of another Region",
            "arn:aws:config:us-west-2:000000000000:connector/azure-tenant-0",
        ),
    ];

    for operation in ["GetConnector", "DeleteConnector"] {
        for (case, arn) in invalid {
            let (status, code) = emulator.refused(operation, &json!({ "Arn": arn })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {operation:?} / {case:?}: expected HTTP 400 ValidationException, \
                 got {status} {code}"
            );
        }
        for (case, arn) in unknown {
            let (status, code) = emulator.refused(operation, &json!({ "Arn": arn })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation:?} / {case:?}: expected HTTP 400 \
                 ResourceNotFoundException, got {status} {code}"
            );
        }
    }

    // None of the refusals above changed the store.
    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        1,
        "a refused request must not change what a listing reports, got {arns:?}"
    );
}

#[tokio::test]
async fn one_provider_configuration_has_at_most_one_connector() {
    let emulator = Emulator::new();
    let arn = put_connector(&emulator, "tenant-0", "client-0").await;

    // Rows are second requests for a configuration already connected: the same
    // configuration, and the same tenant with another client identifier — the tenant
    // is what the connector connects to, so both name the connector that exists.
    let cases: [(&str, &str, &str); 2] = [
        ("the same configuration", "tenant-0", "client-0"),
        ("the same tenant, another client", "tenant-0", "client-1"),
    ];
    for (case, tenant, client) in cases {
        let (status, code) = emulator
            .refused(
                "PutConnector",
                &json!({ "ConnectorConfiguration": azure_configuration(tenant, client) }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConflictException"),
            "case {case:?}: expected HTTP 400 ConflictException, got {status} {code}"
        );
    }

    // Another tenant is another connector, not a conflict.
    let other = put_connector(&emulator, "tenant-1", "client-0").await;
    assert_ne!(
        other, arn,
        "a connector for another tenant must get its own ARN, got {other:?}"
    );

    // Concurrent requests for one configuration have a single winner: the check and
    // the insert are one step, so the store never holds two connectors for it.
    let request = json!({
        "ConnectorConfiguration": azure_configuration("tenant-concurrent", "client")
    });
    let (first, second, third, fourth) = tokio::join!(
        emulator.call("PutConnector", &request),
        emulator.call("PutConnector", &request),
        emulator.call("PutConnector", &request),
        emulator.call("PutConnector", &request),
    );
    let outcomes = [first, second, third, fourth];
    let winners = outcomes
        .iter()
        .filter(|(status, _)| *status == 200)
        .count();
    assert_eq!(
        winners, 1,
        "case \"concurrent requests for one configuration\": exactly one must win, got \
         {outcomes:?}"
    );

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        3,
        "the store must hold one connector per configuration, got {arns:?}"
    );
}

#[tokio::test]
async fn an_identifier_with_padding_is_refused_rather_than_normalised() {
    let emulator = Emulator::new();
    // The connector's name is derived from the tenant identifier, so an identifier is
    // stored exactly as the caller sent it: padding is refused, not trimmed away into
    // a different identifier. Rows differ along which identifier carries the padding
    // and along the whitespace itself.
    let cases: [(&str, &str, &str); 4] = [
        ("leading space on the tenant", " tenant-0", "client-0"),
        ("trailing space on the tenant", "tenant-0 ", "client-0"),
        ("tab around the client", "\ttenant-0", "client-0"),
        ("newline after the tenant", "tenant-0\n", "client-0"),
    ];
    for (case, tenant, client) in cases {
        let (status, code) = emulator
            .refused(
                "PutConnector",
                &json!({ "ConnectorConfiguration": azure_configuration(tenant, client) }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}

#[tokio::test]
async fn a_connector_is_served_only_to_the_region_it_belongs_to() {
    // One account's store reached from two Regions: a connector is a regional
    // resource, so each Region sees only its own.
    let east = Emulator::in_region("us-east-1");
    let west = east.in_other_region("us-west-2");

    let east_arn = put_connector(&east, "tenant-0", "client").await;
    // The same tenant in another Region is another connector, not a conflict.
    let west_arn = put_connector(&west, "tenant-0", "client").await;
    assert_ne!(
        east_arn, west_arn,
        "each Region must mint its own connector ARN, got {east_arn:?} and {west_arn:?}"
    );

    for (case, emulator, own, other) in [
        ("us-east-1 caller", &east, &east_arn, &west_arn),
        ("us-west-2 caller", &west, &west_arn, &east_arn),
    ] {
        let (arns, _) = list_connector_arns(emulator, 0).await;
        assert_eq!(
            arns,
            vec![own.clone()],
            "case {case:?}: a listing must report only this Region's connectors, got \
             {arns:?}"
        );
        let read = emulator
            .ok("GetConnector", &json!({ "Arn": own }))
            .await;
        assert_eq!(
            read["Connector"]["arn"].as_str(),
            Some(own.as_str()),
            "case {case:?}: this Region's connector must be readable, got {read}"
        );
        for operation in ["GetConnector", "DeleteConnector"] {
            let (status, code) = emulator.refused(operation, &json!({ "Arn": other })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} / {operation:?}: another Region's connector must be \
                 answered as missing, got {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn a_configuration_naming_no_provider_is_refused() {
    let emulator = Emulator::new();
    // `ConnectorConfiguration`: "You must specify exactly one provider
    // configuration." Rows differ along HOW the configuration fails to name one, and
    // along which identifier of a named provider is blank.
    let cases: [(&str, Value); 7] = [
        ("no provider configuration", json!({})),
        (
            "a blank tenant identifier",
            azure_configuration(" ", "client"),
        ),
        (
            "a blank client identifier",
            azure_configuration("tenant-0", "\t"),
        ),
        ("both identifiers blank", azure_configuration(" ", " ")),
        // The connector's name is the resource portion of its ARN, so an identifier
        // that cannot be part of one would mint a connector nothing could name
        // afterwards. `AzureTenantIdentifier` constrains only its length (1..128), so
        // these reach the handler.
        (
            "a tenant identifier carrying the ARN resource separator",
            azure_configuration("tenant/0", "client"),
        ),
        (
            "a tenant identifier that is only the separator",
            azure_configuration("/", "client"),
        ),
        (
            "a tenant identifier ending in the separator",
            azure_configuration("tenant-0/", "client"),
        ),
    ];

    for (case, configuration) in cases {
        let (status, code) = emulator
            .refused(
                "PutConnector",
                &json!({ "ConnectorConfiguration": configuration }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
    }

    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert!(
        arns.is_empty(),
        "a refused PutConnector must create nothing, got {arns:?}"
    );
}

#[tokio::test]
async fn the_connector_store_is_capped_at_this_accounts_quota() {
    let emulator = Emulator::new();
    // The quota this emulator enforces over the store, documented on
    // `crate::state::CONNECTORS_QUOTA`: the `ConnectorSummaries` length maximum.
    const QUOTA: usize = 100;

    for index in 0..QUOTA {
        put_connector(&emulator, &format!("tenant-{index:03}"), "client").await;
    }

    let (status, code) = emulator
        .refused(
            "PutConnector",
            &json!({
                "ConnectorConfiguration": azure_configuration("tenant-past-the-quota", "client")
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "MaxNumberOfConnectorsExceededException"),
        "case \"one past the quota\": expected HTTP 400 \
         MaxNumberOfConnectorsExceededException, got {status} {code}"
    );

    // The refusal left the stored connectors alone, and every one is still listed.
    let (arns, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        arns.len(),
        QUOTA,
        "the store must still hold the quota's worth of connectors, got {} of them",
        arns.len()
    );

    // Deleting one makes room for exactly one more.
    emulator
        .ok("DeleteConnector", &json!({ "Arn": arns[0].clone() }))
        .await;
    let created = put_connector(&emulator, "tenant-past-the-quota", "client").await;
    assert!(
        !arns.contains(&created),
        "case \"after a delete\": the connector created must be a new one, got \
         {created:?}"
    );
    let (after, _) = list_connector_arns(&emulator, 0).await;
    assert_eq!(
        after.len(),
        QUOTA,
        "case \"after a delete\": the store must be back at the quota, got {} \
         connector(s)",
        after.len()
    );
}
