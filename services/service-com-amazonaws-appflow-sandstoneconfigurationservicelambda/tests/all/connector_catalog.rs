// SPDX-License-Identifier: CC0-1.0

//! `DescribeConnectors`, end to end over HTTP.
//!
//! The rule under test: the operation reports the connector CATALOG — one
//! `ConnectorConfiguration` per connector type the request asks about (every type when
//! it names none), stating what a flow may do with that connector, together with the
//! connectors this account registered, read from the store — and it pages that answer
//! with `maxResults` and `nextToken`.
//!
//! The capabilities are asserted over a TABLE spanning the classes a connector can fall
//! into (read-only, write-only, both; event-driven or not; PrivateLink or not), and the
//! derived members are asserted as INVARIANTS of the whole catalog rather than as the
//! values of any one connector: every destination a source offers must itself report
//! that it can be a destination, a connector that a flow cannot read from drives no
//! flow, and paging must report every connector exactly once whatever page size is
//! asked for.
//!
//! The harness (`call`, `Answer`) is the one `flow_lifecycle` already drives the service
//! with, so these cases reach the service exactly the way a client does.

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::ConnectorDetail;
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    encode_cursor, service_state, CONNECTOR_TYPE_CUSTOM, DEFAULT_CONNECTOR_PAGE_SIZE, FLOW_CURSOR,
    SCHEDULE_FREQUENCIES, VENDED_CONNECTORS,
};

use super::flow_lifecycle::{appflow, appflow_with_engine, call, Answer};

/// The operation's HTTP binding
/// (`smithy_query(query="operation", operation="DescribeConnectors")`: POST
/// `/describe-connectors`).
const DESCRIBE_CONNECTORS: &str = "/describe-connectors";

/// What a flow may do with one connector — the capabilities the configuration for that
/// connector type has to state.
#[derive(Debug)]
struct Capability {
    connector_type: &'static str,
    can_use_as_source: bool,
    can_use_as_destination: bool,
    /// Whether a change in the source application can run a flow, which is what makes
    /// the `Event` trigger type available.
    runs_on_an_event: bool,
    is_private_link_enabled: bool,
}

/// One row per capability class, including connector types this suite's subject was not
/// reported against (`Veeva`, `Pardot`, `Honeycode`, …) so the rule is not asserted only
/// where it was first noticed.
const CAPABILITIES: &[Capability] = &[
    // Read and written, event-driven, reachable over PrivateLink.
    Capability {
        connector_type: "Salesforce",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: true,
        is_private_link_enabled: true,
    },
    // Read and written, not event-driven.
    Capability {
        connector_type: "S3",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "Zendesk",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    // Read and written, reachable over PrivateLink but not event-driven.
    Capability {
        connector_type: "SAPOData",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: true,
    },
    // Read only.
    Capability {
        connector_type: "Amplitude",
        can_use_as_source: true,
        can_use_as_destination: false,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "Pardot",
        can_use_as_source: true,
        can_use_as_destination: false,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "Veeva",
        can_use_as_source: true,
        can_use_as_destination: false,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    // Written only.
    Capability {
        connector_type: "EventBridge",
        can_use_as_source: false,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "Redshift",
        can_use_as_source: false,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "Honeycode",
        can_use_as_source: false,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
];

/// Every connector type the catalog vends, sorted.
fn vended_types() -> Vec<String> {
    let mut types: Vec<String> = VENDED_CONNECTORS
        .iter()
        .map(|connector| connector.connector_type.to_string())
        .collect();
    types.sort();
    types
}

/// The `connectorConfigurations` of an answer, as a JSON object.
fn configurations(answer: &Answer) -> serde_json::Map<String, Value> {
    answer
        .get("connectorConfigurations")
        .as_object()
        .cloned()
        .unwrap_or_default()
}

/// A boolean member of one configuration (absent reads as `false`, the member's modelled
/// default).
fn flag(configuration: &Value, member: &str) -> bool {
    configuration
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A list member of one configuration, as strings.
fn list(configuration: &Value, member: &str) -> Vec<String> {
    configuration
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A connector detail of the kind `RegisterConnector` stores, so a read can be seen to
/// answer from the store.
fn registered(label: &str) -> ConnectorDetail {
    ConnectorDetail {
        connector_label: Some(label.to_string()),
        connector_name: Some(label.to_string()),
        connector_owner: Some("an independent publisher".to_string()),
        connector_version: Some("1.0".to_string()),
        connector_type: Some(CONNECTOR_TYPE_CUSTOM.to_string()),
        connector_provisioning_type: Some("LAMBDA".to_string()),
        supported_data_transfer_types: Some(vec!["RECORD".to_string()]),
        ..ConnectorDetail::default()
    }
}

/// Describe the types `connector_types` with a page big enough for all of them.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    connector_types: &[&str],
) -> Answer {
    let body = json!({ "connectorTypes": connector_types, "maxResults": 100 });
    call(router, DESCRIBE_CONNECTORS, &body).await
}

#[tokio::test]
async fn every_connector_reports_the_capabilities_a_flow_may_use_it_with() {
    let router = appflow();
    let requested: Vec<&str> = CAPABILITIES
        .iter()
        .map(|case| case.connector_type)
        .collect();
    let answer = describe(&router, &requested).await;
    assert!(
        answer.is_ok(),
        "case {requested:?}: expected HTTP 200, got {answer:?}"
    );

    let reported = configurations(&answer);
    assert_eq!(
        reported.len(),
        CAPABILITIES.len(),
        "case {requested:?}: expected one configuration per requested type, got {:?}",
        reported.keys().collect::<Vec<_>>()
    );

    let frequencies: Vec<String> = SCHEDULE_FREQUENCIES
        .iter()
        .map(|frequency| (*frequency).to_string())
        .collect();

    for case in CAPABILITIES {
        let Some(configuration) = reported.get(case.connector_type) else {
            panic!(
                "case {case:?}: expected a configuration for this type, got {:?}",
                reported.keys().collect::<Vec<_>>()
            );
        };

        assert_eq!(
            configuration.get("connectorType").and_then(Value::as_str),
            Some(case.connector_type),
            "case {case:?}: a configuration must name the connector type it describes, got {configuration:?}"
        );
        assert_eq!(
            flag(configuration, "canUseAsSource"),
            case.can_use_as_source,
            "case {case:?}: canUseAsSource is {:?}",
            flag(configuration, "canUseAsSource")
        );
        assert_eq!(
            flag(configuration, "canUseAsDestination"),
            case.can_use_as_destination,
            "case {case:?}: canUseAsDestination is {:?}",
            flag(configuration, "canUseAsDestination")
        );
        assert_eq!(
            flag(configuration, "isPrivateLinkEnabled"),
            case.is_private_link_enabled,
            "case {case:?}: isPrivateLinkEnabled is {:?}",
            flag(configuration, "isPrivateLinkEnabled")
        );

        // A flow is driven by the connector it reads FROM, so only a source reports the
        // trigger types that can run it and the frequencies a schedule may use — in the
        // order the model declares each enum.
        let mut expected_triggers: Vec<String> = Vec::new();
        if case.can_use_as_source {
            expected_triggers.push("Scheduled".to_string());
            if case.runs_on_an_event {
                expected_triggers.push("Event".to_string());
            }
            expected_triggers.push("OnDemand".to_string());
        }
        assert_eq!(
            list(configuration, "supportedTriggerTypes"),
            expected_triggers,
            "case {case:?}: supportedTriggerTypes is {:?}",
            list(configuration, "supportedTriggerTypes")
        );
        assert_eq!(
            list(configuration, "supportedSchedulingFrequencies"),
            if case.can_use_as_source {
                frequencies.clone()
            } else {
                Vec::new()
            },
            "case {case:?}: supportedSchedulingFrequencies is {:?}",
            list(configuration, "supportedSchedulingFrequencies")
        );
        // Only a source offers destinations to send records to.
        assert_eq!(
            !list(configuration, "supportedDestinationConnectors").is_empty(),
            case.can_use_as_source,
            "case {case:?}: supportedDestinationConnectors is {:?}",
            list(configuration, "supportedDestinationConnectors")
        );

        // The capabilities a connector DECLARES to AppFlow belong to a registration, and
        // nobody registered a connector AppFlow vends, so its configuration carries none
        // of them.
        for declared in [
            "supportedOperators",
            "supportedWriteOperations",
            "supportedDataTransferTypes",
        ] {
            assert!(
                configuration.get(declared).is_none(),
                "case {case:?}: a vended connector declares no {declared}, got {configuration:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_destination_a_source_offers_can_itself_be_a_destination() {
    let router = appflow();

    // Ask a connector a flow can read from which destinations it offers, then ask those
    // connectors themselves: the two answers must agree, for every source in the
    // catalog.
    let sources: Vec<&str> = VENDED_CONNECTORS
        .iter()
        .filter(|connector| connector.can_use_as_source)
        .map(|connector| connector.connector_type)
        .collect();
    let answer = describe(&router, &sources).await;
    assert!(
        answer.is_ok(),
        "case {sources:?}: expected HTTP 200, got {answer:?}"
    );
    let reported = configurations(&answer);

    let mut offered: Vec<String> = Vec::new();
    for source in &sources {
        let configuration = reported
            .get(*source)
            .unwrap_or_else(|| panic!("case {source}: expected a configuration, got none"));
        let destinations = list(configuration, "supportedDestinationConnectors");
        assert!(
            !destinations.is_empty(),
            "case {source}: a source must offer at least one destination, got {destinations:?}"
        );
        offered.extend(destinations);
    }
    offered.sort();
    offered.dedup();

    // A flow can write to a connector an ACCOUNT registered as well as to one AppFlow
    // vends, so the custom-connector type is offered too — and it is the one type the
    // vended catalog has no entry for, so it is not asked back here: a registration is
    // identified by its label, and `connectorConfigurations` is keyed by type.
    assert!(
        offered.iter().any(|offered| offered == CONNECTOR_TYPE_CUSTOM),
        "a flow can write to a registered connector, but the offered destinations are {offered:?}"
    );
    let offered_types: Vec<&str> = offered
        .iter()
        .filter(|offered| *offered != CONNECTOR_TYPE_CUSTOM)
        .map(String::as_str)
        .collect();
    let answer = describe(&router, &offered_types).await;
    assert!(
        answer.is_ok(),
        "case {offered_types:?}: expected HTTP 200, got {answer:?}"
    );
    let reported = configurations(&answer);
    for destination in &offered_types {
        let configuration = reported
            .get(*destination)
            .unwrap_or_else(|| panic!("case {destination}: expected a configuration, got none"));
        assert!(
            flag(configuration, "canUseAsDestination"),
            "case {destination}: offered as a destination, but its own configuration is {configuration:?}"
        );
    }
}

#[tokio::test]
async fn the_whole_catalog_is_reported_exactly_once_whatever_the_page_size() {
    let router = appflow();

    // A request that names no type asks about every connector there is, so the listing
    // is longer than the default page and has to be followed.
    for max_results in [None, Some(1), Some(5), Some(23), Some(100)] {
        let page_bound = max_results.unwrap_or(DEFAULT_CONNECTOR_PAGE_SIZE as i32) as usize;
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({});
            if let Some(max_results) = max_results {
                body["maxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                body["nextToken"] = json!(token);
            }
            let answer = call(&router, DESCRIBE_CONNECTORS, &body).await;
            assert!(
                answer.is_ok(),
                "case maxResults={max_results:?} page {pages}: expected HTTP 200, got {answer:?}"
            );
            let page = configurations(&answer);
            assert!(
                page.len() <= page_bound,
                "case maxResults={max_results:?} page {pages}: a page must hold at most {page_bound} configurations, got {}",
                page.len()
            );
            seen.extend(page.keys().cloned());
            pages += 1;
            assert!(
                pages <= VENDED_CONNECTORS.len() + 2,
                "case maxResults={max_results:?}: paging did not end, {pages} pages so far"
            );
            match answer.get("nextToken").as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        seen.sort();
        assert_eq!(
            seen,
            vended_types(),
            "case maxResults={max_results:?}: expected every vended connector exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn only_the_connector_types_the_request_names_are_reported() {
    let router = appflow();
    let every_type = vended_types();

    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("one type", vec!["S3"], vec!["S3".to_string()]),
        (
            "two types, named in the reverse order",
            vec!["Salesforce", "S3"],
            vec!["S3".to_string(), "Salesforce".to_string()],
        ),
        (
            "the same type twice",
            vec!["Veeva", "Veeva"],
            vec!["Veeva".to_string()],
        ),
        (
            "a type with no scheduling of its own",
            vec!["Upsolver"],
            vec!["Upsolver".to_string()],
        ),
        ("no type at all", vec![], every_type.clone()),
    ];

    for (case, requested, expected) in cases {
        let answer = describe(&router, &requested).await;
        assert!(
            answer.is_ok(),
            "case {case} ({requested:?}): expected HTTP 200, got {answer:?}"
        );
        let mut reported: Vec<String> = configurations(&answer).keys().cloned().collect();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case} ({requested:?}): reported {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_cursor_this_listing_did_not_issue_is_refused() {
    let router = appflow();

    for (case, token) in [
        (
            "a cursor another listing minted",
            encode_cursor(FLOW_CURSOR, "a-flow"),
        ),
        ("a token that does not decode", "not~a~cursor".to_string()),
    ] {
        let answer = call(
            &router,
            DESCRIBE_CONNECTORS,
            &json!({ "nextToken": token.clone() }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case} ({token}): expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error, "ValidationException",
            "case {case} ({token}): expected a ValidationException, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_page_outside_the_modelled_bounds_is_refused() {
    let router = appflow();

    // The published bounds of `maxResults` (range 1..100), and one past the top.
    for (case, max_results, expected_status) in [
        ("the smallest page the model allows", 1, 200),
        ("the largest page the model allows", 100, 200),
        ("one past the largest page", 101, 400),
        ("no page at all", 0, 400),
    ] {
        let answer = call(
            &router,
            DESCRIBE_CONNECTORS,
            &json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case} (maxResults={max_results}): got {answer:?}"
        );
    }
}

#[tokio::test]
async fn the_connectors_this_account_registered_are_reported_from_the_store() {
    let (router, engine) = appflow_with_engine();
    let labels = ["AcmeCrm", "BetaWarehouse"];
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        for label in labels {
            store
                .registered_connectors
                .insert(label.to_string(), registered(label));
        }
    }

    // The custom-connector type stands for the connectors this account registered: they
    // come back as the details they were registered with, and the configurations map —
    // which is keyed by TYPE and so cannot name a label — has no entry for it.
    let answer = describe(&router, &[CONNECTOR_TYPE_CUSTOM]).await;
    assert!(
        answer.is_ok(),
        "case {CONNECTOR_TYPE_CUSTOM}: expected HTTP 200, got {answer:?}"
    );
    let reported: Vec<String> = answer
        .get("connectors")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|detail| {
            detail
                .get("connectorLabel")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        reported,
        labels.map(str::to_string).to_vec(),
        "case {CONNECTOR_TYPE_CUSTOM}: expected the registered labels, got {reported:?}"
    );
    assert!(
        configurations(&answer).is_empty(),
        "case {CONNECTOR_TYPE_CUSTOM}: expected no configuration keyed by type, got {:?}",
        configurations(&answer).keys().collect::<Vec<_>>()
    );

    // A request about a vended type asks nothing about them.
    let answer = describe(&router, &["S3"]).await;
    assert!(
        answer
            .get("connectors")
            .as_array()
            .is_none_or(Vec::is_empty),
        "case S3: a vended type reports no registered connector, got {answer:?}"
    );

    // A request that names no type asks about them too.
    let answer = describe(&router, &[]).await;
    assert_eq!(
        answer
            .get("connectors")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        labels.len(),
        "case no type at all: expected the registered connectors, got {answer:?}"
    );

    // Reading the catalog is a report, not a change: the store still holds exactly what
    // was registered.
    let state = service_state(&*engine).expect("the AppFlow store is on this engine");
    let store = state.lock();
    let stored: Vec<String> = store.registered_connectors.keys().cloned().collect();
    assert_eq!(
        stored,
        labels.map(str::to_string).to_vec(),
        "after describing the catalog the store holds {stored:?}"
    );
}
