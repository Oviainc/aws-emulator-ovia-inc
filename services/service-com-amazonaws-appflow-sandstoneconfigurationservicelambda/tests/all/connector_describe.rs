// SPDX-License-Identifier: CC0-1.0

//! `DescribeConnector`, end to end over HTTP.
//!
//! The rule under test: the operation reports the configuration of the ONE connector the
//! request names — the service's own catalog entry for a connector AppFlow vends, or the
//! registration this account holds under the `connectorLabel` the request names for the
//! custom-connector type — and it refuses a request that names nothing describable: a
//! label no registration holds, or the custom type with no label at all.
//!
//! The vended half is asserted over a TABLE spanning the capability classes a connector
//! can fall into (read-only, write-only, both; event-driven or not; PrivateLink or not),
//! and then as an INVARIANT over the WHOLE catalog: describing one connector must agree
//! with what the catalog listing reports for that same type, so neither answer can drift
//! from the other. The registered half is a table spanning the connection modes and the
//! data-transfer types a registration can declare, each one seeded in the store and read
//! back through this operation.
//!
//! The harness (`call`, `Answer`) is the one `flow_lifecycle` already drives the service
//! with, so these cases reach the service exactly the way a client does.

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::ConnectorDetail;
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    service_state, CONNECTOR_MODE_DESTINATION, CONNECTOR_MODE_SOURCE, CONNECTOR_TYPE_CUSTOM,
    VENDED_CONNECTORS,
};

use super::flow_lifecycle::{appflow, appflow_with_engine, call, Answer, ACCOUNT};

/// The operation's HTTP binding
/// (`smithy_query(query="operation", operation="DescribeConnector")`: POST
/// `/describe-connector`).
const DESCRIBE_CONNECTOR: &str = "/describe-connector";

/// The catalog listing, used to check the two answers agree
/// (`smithy_query(query="operation", operation="DescribeConnectors")`).
const DESCRIBE_CONNECTORS: &str = "/describe-connectors";

/// What a flow may do with one connector AppFlow vends — the capabilities the
/// configuration for that type has to state.
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
/// reported against (`Veeva`, `Upsolver`, `LookoutMetrics`, …) so the rule is not
/// asserted only where it was first noticed. The types come from the model's own
/// `ConnectorType` enum (`smithy_query(query="shape", shape="ConnectorType")`).
const CAPABILITIES: &[Capability] = &[
    // Read and written, not event-driven: the connector the recorded call asked about.
    Capability {
        connector_type: "S3",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    // Read and written, event-driven, reachable over PrivateLink.
    Capability {
        connector_type: "Salesforce",
        can_use_as_source: true,
        can_use_as_destination: true,
        runs_on_an_event: true,
        is_private_link_enabled: true,
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
        connector_type: "Veeva",
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
    // Written only.
    Capability {
        connector_type: "Upsolver",
        can_use_as_source: false,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
    Capability {
        connector_type: "LookoutMetrics",
        can_use_as_source: false,
        can_use_as_destination: true,
        runs_on_an_event: false,
        is_private_link_enabled: false,
    },
];

/// One registration a caller holds, and the modes it declares to AppFlow.
#[derive(Debug)]
struct Registration {
    label: &'static str,
    /// The `ConnectorMode`s the connector supports: the direction a flow may use it in.
    modes: &'static [&'static str],
    /// The `SupportedDataTransferType`s it moves (`RECORD`, `FILE`, or both — the only
    /// values the model declares, `smithy_query(query="shape",
    /// shape="SupportedDataTransferType")`).
    data_transfer_types: &'static [&'static str],
    version: &'static str,
}

/// One row per class a registration can fall into: read-only, write-only and both, each
/// moving records, files or both.
const REGISTRATIONS: &[Registration] = &[
    Registration {
        label: "AcmeCrm",
        modes: &[CONNECTOR_MODE_SOURCE],
        data_transfer_types: &["RECORD"],
        version: "1.0",
    },
    Registration {
        label: "BetaWarehouse",
        modes: &[CONNECTOR_MODE_DESTINATION],
        data_transfer_types: &["RECORD", "FILE"],
        version: "2.17.3",
    },
    Registration {
        label: "Gamma.Archive-9",
        modes: &[CONNECTOR_MODE_SOURCE, CONNECTOR_MODE_DESTINATION],
        data_transfer_types: &["FILE"],
        version: "0.1",
    },
    // A registration that declares no mode at all: a flow can do nothing with it.
    Registration {
        label: "DeltaUnusable",
        modes: &[],
        data_transfer_types: &["RECORD"],
        version: "1",
    },
];

/// The `connectorConfiguration` of an answer.
fn configuration(answer: &Answer) -> &Value {
    answer.get("connectorConfiguration")
}

/// A boolean member of a configuration (absent reads as `false`, the member's modelled
/// default).
fn flag(configuration: &Value, member: &str) -> bool {
    configuration
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A list member of a configuration, as strings.
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

/// A string member of a configuration.
fn member(configuration: &Value, member: &str) -> String {
    configuration
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The registration a caller holds, as `RegisterConnector` would store it.
fn registered(case: &Registration) -> ConnectorDetail {
    ConnectorDetail {
        connector_label: Some(case.label.to_string()),
        connector_name: Some(format!("{} connector", case.label)),
        connector_description: Some("A connector this account registered".to_string()),
        connector_owner: Some("an independent publisher".to_string()),
        connector_version: Some(case.version.to_string()),
        connector_type: Some(CONNECTOR_TYPE_CUSTOM.to_string()),
        connector_provisioning_type: Some("LAMBDA".to_string()),
        connector_modes: Some(case.modes.iter().map(|mode| (*mode).to_string()).collect()),
        supported_data_transfer_types: Some(
            case.data_transfer_types
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
        ),
        registered_by: Some(format!("arn:aws:iam::{ACCOUNT}:user/publisher")),
        ..ConnectorDetail::default()
    }
}

#[tokio::test]
async fn a_vended_connector_is_described_with_the_capabilities_a_flow_may_use_it_with() {
    let router = appflow();

    for case in CAPABILITIES {
        let answer = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": case.connector_type }),
        )
        .await;
        assert!(
            answer.is_ok(),
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        let reported = configuration(&answer).clone();

        assert_eq!(
            member(&reported, "connectorType"),
            case.connector_type,
            "case {case:?}: a configuration must name the connector it describes, got {reported:?}"
        );
        assert_eq!(
            flag(&reported, "canUseAsSource"),
            case.can_use_as_source,
            "case {case:?}: canUseAsSource is {:?}",
            flag(&reported, "canUseAsSource")
        );
        assert_eq!(
            flag(&reported, "canUseAsDestination"),
            case.can_use_as_destination,
            "case {case:?}: canUseAsDestination is {:?}",
            flag(&reported, "canUseAsDestination")
        );
        assert_eq!(
            flag(&reported, "isPrivateLinkEnabled"),
            case.is_private_link_enabled,
            "case {case:?}: isPrivateLinkEnabled is {:?}",
            flag(&reported, "isPrivateLinkEnabled")
        );

        // A flow is driven by the connector it reads FROM, so only a source reports the
        // trigger types that can run it, in the order the model declares `TriggerType`.
        let mut expected_triggers: Vec<String> = Vec::new();
        if case.can_use_as_source {
            expected_triggers.push("Scheduled".to_string());
            if case.runs_on_an_event {
                expected_triggers.push("Event".to_string());
            }
            expected_triggers.push("OnDemand".to_string());
        }
        assert_eq!(
            list(&reported, "supportedTriggerTypes"),
            expected_triggers,
            "case {case:?}: supportedTriggerTypes is {:?}",
            list(&reported, "supportedTriggerTypes")
        );
        assert_eq!(
            !list(&reported, "supportedDestinationConnectors").is_empty(),
            case.can_use_as_source,
            "case {case:?}: supportedDestinationConnectors is {:?}",
            list(&reported, "supportedDestinationConnectors")
        );

        // Nobody registered a connector AppFlow vends, so it carries neither the
        // registration members nor the capabilities a registration declares.
        for registration_member in [
            "connectorLabel",
            "connectorArn",
            "registeredAt",
            "supportedOperators",
            "supportedWriteOperations",
            "supportedDataTransferTypes",
        ] {
            assert!(
                reported.get(registration_member).is_none(),
                "case {case:?}: a vended connector carries no {registration_member}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn describing_one_connector_agrees_with_the_catalog_listing() {
    let router = appflow();

    // Over the WHOLE catalog, not just the rows above: the two operations report the
    // same service-wide catalog, so neither may answer what the other does not.
    for connector in VENDED_CONNECTORS.iter() {
        let case = connector.connector_type;
        let one = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": case }),
        )
        .await;
        assert!(one.is_ok(), "case {case}: expected HTTP 200, got {one:?}");

        let listed = call(
            &router,
            DESCRIBE_CONNECTORS,
            &json!({ "connectorTypes": [case], "maxResults": 1 }),
        )
        .await;
        assert!(
            listed.is_ok(),
            "case {case}: expected HTTP 200 from the listing, got {listed:?}"
        );
        let from_listing = listed
            .get("connectorConfigurations")
            .get(case)
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            configuration(&one),
            &from_listing,
            "case {case}: describing one connector reported {:?} while the listing reported {from_listing:?}",
            configuration(&one)
        );
    }
}

#[tokio::test]
async fn a_label_does_not_change_what_a_vended_connector_reports() {
    let router = appflow();

    // A label identifies a registration, and is "only needed if calling for
    // CUSTOMCONNECTOR connector type" (`smithy_query(query="doc",
    // shape="DescribeConnectorRequest", member="connectorLabel")`), so it cannot change
    // what a connector AppFlow vends reports.
    for case in ["S3", "Salesforce", "EventBridge"] {
        let without = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": case }),
        )
        .await;
        let with = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": case, "connectorLabel": "AcmeCrm" }),
        )
        .await;
        assert!(
            with.is_ok(),
            "case {case} with a label: expected HTTP 200, got {with:?}"
        );
        assert_eq!(
            configuration(&with),
            configuration(&without),
            "case {case}: a label changed the answer to {:?}",
            configuration(&with)
        );
    }
}

#[tokio::test]
async fn a_registered_connector_is_described_from_the_store() {
    let (router, engine) = appflow_with_engine();
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        for case in REGISTRATIONS {
            store
                .registered_connectors
                .insert(case.label.to_string(), registered(case));
        }
    }

    for case in REGISTRATIONS {
        let answer = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": CONNECTOR_TYPE_CUSTOM, "connectorLabel": case.label }),
        )
        .await;
        assert!(
            answer.is_ok(),
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        let reported = configuration(&answer).clone();

        // The members the registration itself holds come back as they were stored.
        assert_eq!(
            member(&reported, "connectorLabel"),
            case.label,
            "case {case:?}: reported {reported:?}"
        );
        assert_eq!(
            member(&reported, "connectorType"),
            CONNECTOR_TYPE_CUSTOM,
            "case {case:?}: reported {reported:?}"
        );
        assert_eq!(
            member(&reported, "connectorName"),
            format!("{} connector", case.label),
            "case {case:?}: reported {reported:?}"
        );
        assert_eq!(
            member(&reported, "connectorVersion"),
            case.version,
            "case {case:?}: reported {reported:?}"
        );
        assert_eq!(
            member(&reported, "connectorProvisioningType"),
            "LAMBDA",
            "case {case:?}: reported {reported:?}"
        );
        assert_eq!(
            list(&reported, "connectorModes"),
            case.modes
                .iter()
                .map(|mode| (*mode).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: connectorModes is {:?}",
            list(&reported, "connectorModes")
        );
        assert_eq!(
            list(&reported, "supportedDataTransferTypes"),
            case.data_transfer_types
                .iter()
                .map(|kind| (*kind).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: supportedDataTransferTypes is {:?}",
            list(&reported, "supportedDataTransferTypes")
        );
        assert_eq!(
            member(&reported, "connectorArn"),
            format!(
                "arn:aws:appflow:us-east-1:{ACCOUNT}:connector/{}",
                case.label
            ),
            "case {case:?}: reported {reported:?}"
        );

        // What a flow may do with it is the modes it declared, and nothing else.
        let declares = |mode: &str| case.modes.contains(&mode);
        assert_eq!(
            flag(&reported, "canUseAsSource"),
            declares(CONNECTOR_MODE_SOURCE),
            "case {case:?}: canUseAsSource is {:?}",
            flag(&reported, "canUseAsSource")
        );
        assert_eq!(
            flag(&reported, "canUseAsDestination"),
            declares(CONNECTOR_MODE_DESTINATION),
            "case {case:?}: canUseAsDestination is {:?}",
            flag(&reported, "canUseAsDestination")
        );
        assert_eq!(
            !list(&reported, "supportedWriteOperations").is_empty(),
            declares(CONNECTOR_MODE_DESTINATION),
            "case {case:?}: supportedWriteOperations is {:?}",
            list(&reported, "supportedWriteOperations")
        );
        assert_eq!(
            !list(&reported, "supportedTriggerTypes").is_empty(),
            declares(CONNECTOR_MODE_SOURCE),
            "case {case:?}: supportedTriggerTypes is {:?}",
            list(&reported, "supportedTriggerTypes")
        );
    }

    // Describing a connector is a report, not a change: the store still holds exactly
    // what was registered.
    let state = service_state(&*engine).expect("the AppFlow store is on this engine");
    let store = state.lock();
    let mut stored: Vec<String> = store.registered_connectors.keys().cloned().collect();
    stored.sort();
    let mut expected: Vec<String> = REGISTRATIONS
        .iter()
        .map(|case| case.label.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "after describing the registered connectors the store holds {stored:?}"
    );
}

#[tokio::test]
async fn a_request_that_names_nothing_describable_is_refused() {
    let (router, engine) = appflow_with_engine();
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        store
            .registered_connectors
            .insert("AcmeCrm".to_string(), registered(&REGISTRATIONS[0]));
    }

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "the custom type with no label, which identifies no registration",
            json!({ "connectorType": CONNECTOR_TYPE_CUSTOM }),
            400,
            "ValidationException",
        ),
        (
            "a label no registration holds",
            json!({ "connectorType": CONNECTOR_TYPE_CUSTOM, "connectorLabel": "NoSuchConnector" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a label that differs from a registered one only in case",
            json!({ "connectorType": CONNECTOR_TYPE_CUSTOM, "connectorLabel": "acmecrm" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a connector type the model does not declare",
            json!({ "connectorType": "NotAConnectorType" }),
            400,
            "ValidationException",
        ),
        (
            "no connector type at all, which the model requires",
            json!({ "connectorLabel": "AcmeCrm" }),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_error) in cases {
        let answer = call(&router, DESCRIBE_CONNECTOR, &body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case} ({body}): got {answer:?}"
        );
        assert_eq!(
            answer.error, expected_error,
            "case {case} ({body}): got {answer:?}"
        );
    }

    // The registration a refused request named is still the only one in the store.
    let state = service_state(&*engine).expect("the AppFlow store is on this engine");
    let store = state.lock();
    let stored: Vec<String> = store.registered_connectors.keys().cloned().collect();
    assert_eq!(
        stored,
        vec!["AcmeCrm".to_string()],
        "after refusing those requests the store holds {stored:?}"
    );
}
