// SPDX-License-Identifier: CC0-1.0

//! The `ConnectorConfiguration` AppFlow reports for ONE connector, end to end over HTTP.
//!
//! The rule under test: every member of a `ConnectorConfiguration` is a property of the
//! connector the request names, taken from that connector's own record — its entry in
//! the catalog of connectors AppFlow vends, or the registration this account holds for a
//! custom connector — and a member that record does not carry is OMITTED rather than
//! filled from a value shared by every connector.
//!
//! So this suite asserts, over the WHOLE catalog rather than the one connector the rule
//! was first noticed on:
//!
//! * each connector type is described by its own record: its own description, and no two
//!   types answering the same configuration;
//! * the capabilities a connector DECLARES to AppFlow (`supportedOperators`,
//!   `supportedWriteOperations`, `supportedDataTransferTypes`) are reported for a
//!   connector an account REGISTERED, whose registration declares them, and left unset
//!   for a connector AppFlow vends, which registered nothing — the same code path, two
//!   records;
//! * a member that applies only to a connector a flow can read FROM is unset on one it
//!   can only write TO;
//! * the destinations a source offers are exactly the connector types a flow can write
//!   to, each one a `ConnectorType` the model declares.
//!
//! The harness (`call`, `Answer`) is the one `flow_lifecycle` already drives the service
//! with, so these cases reach the service exactly the way a client does.

use serde_json::{json, Value};
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::ops::types::ConnectorDetail;
use service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::state::{
    destination_catalog_agrees, service_state, CONNECTOR_MODE_DESTINATION, CONNECTOR_MODE_SOURCE,
    CONNECTOR_TYPE_CUSTOM, DESTINATION_CONNECTORS, VENDED_CONNECTORS,
};

use super::flow_lifecycle::{appflow, appflow_with_engine, call};

/// The operation's HTTP binding
/// (`smithy_query(query="operation", operation="DescribeConnector")`: POST
/// `/describe-connector`).
const DESCRIBE_CONNECTOR: &str = "/describe-connector";

/// Every value of the model's `ConnectorType` enum
/// (`smithy_query(query="shape", shape="ConnectorType")`), which is what
/// `ConnectorTypeList` — the shape of `supportedDestinationConnectors` — is a list over.
const CONNECTOR_TYPES: [&str; 24] = [
    "Salesforce",
    "Singular",
    "Slack",
    "Redshift",
    "S3",
    "Marketo",
    "Googleanalytics",
    "Zendesk",
    "Servicenow",
    "Datadog",
    "Trendmicro",
    "Snowflake",
    "Dynatrace",
    "Infornexus",
    "Amplitude",
    "Veeva",
    "EventBridge",
    "LookoutMetrics",
    "Upsolver",
    "Honeycode",
    "CustomerProfiles",
    "SAPOData",
    "CustomConnector",
    "Pardot",
];

/// The `@length` maximum of `ConnectorDescription`
/// (`smithy_query(query="shape", shape="ConnectorConfiguration")` →
/// `connectorDescription` length 0..2048, pattern `^[\w!@#\-.?,\s]*$`).
const MAX_DESCRIPTION: usize = 2048;

/// The `@length` maximum of `ConnectorTypeList`
/// (`smithy_query(query="shape", shape="ConnectorTypeList")`: length 0..100).
const MAX_CONNECTOR_LIST: usize = 100;

/// The description AppFlow publishes for one connector, for a few connector types
/// chosen across the catalog rather than only the one the symptom was reported on.
///
/// The values are the names AppFlow's own list of supported connectors gives the
/// applications those connectors reach, which is what `connectorDescription` ("A
/// description about the connector") carries.
const DESCRIPTIONS: &[(&str, &str)] = &[
    ("S3", "Amazon S3"),
    ("Salesforce", "Salesforce"),
    ("Googleanalytics", "Google Analytics"),
    ("CustomerProfiles", "Amazon Connect Customer Profiles"),
    ("Pardot", "Salesforce Pardot"),
    ("SAPOData", "SAP OData"),
];

/// The members a `ConnectorConfiguration` carries only for a connector that DECLARED
/// them to AppFlow when it was registered.
const DECLARED_MEMBERS: [&str; 3] = [
    "supportedOperators",
    "supportedWriteOperations",
    "supportedDataTransferTypes",
];

/// The members that describe how a flow READING from a connector runs, which a connector
/// a flow can only write to does not carry.
const SOURCE_MEMBERS: [&str; 3] = [
    "supportedDestinationConnectors",
    "supportedSchedulingFrequencies",
    "supportedTriggerTypes",
];

/// `DescribeConnector` for one connector type, answering its `connectorConfiguration`.
async fn describe(
    router: &smithy_server_runtime::MultiServiceRouter,
    connector_type: &str,
) -> Value {
    let answer = call(
        router,
        DESCRIBE_CONNECTOR,
        &json!({ "connectorType": connector_type }),
    )
    .await;
    assert!(
        answer.is_ok(),
        "case {connector_type}: expected HTTP 200, got {answer:?}"
    );
    answer.get("connectorConfiguration").clone()
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

/// A registration of a custom connector, as `RegisterConnector` stores one: it declares
/// both connection modes and the data transfer types it moves.
fn registered(label: &str, data_transfer_types: &[&str]) -> ConnectorDetail {
    ConnectorDetail {
        connector_label: Some(label.to_string()),
        connector_name: Some(format!("{label} connector")),
        connector_description: Some(format!("The {label} application")),
        connector_type: Some(CONNECTOR_TYPE_CUSTOM.to_string()),
        connector_version: Some("1.0".to_string()),
        connector_provisioning_type: Some("LAMBDA".to_string()),
        connector_modes: Some(vec![
            CONNECTOR_MODE_SOURCE.to_string(),
            CONNECTOR_MODE_DESTINATION.to_string(),
        ]),
        supported_data_transfer_types: Some(
            data_transfer_types
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
        ),
        ..ConnectorDetail::default()
    }
}

#[tokio::test]
async fn each_connector_type_is_described_by_its_own_record() {
    let router = appflow();

    // The whole catalog, so the rule is asserted for every connector and not only for
    // the ones named below.
    let mut descriptions: Vec<String> = Vec::new();
    let mut bodies: Vec<(String, Value)> = Vec::new();
    for connector in VENDED_CONNECTORS.iter() {
        let case = connector.connector_type;
        let reported = describe(&router, case).await;

        assert_eq!(
            reported.get("connectorType").and_then(Value::as_str),
            Some(case),
            "case {case}: a configuration must name the connector it describes, got {reported:?}"
        );

        let description = reported
            .get("connectorDescription")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !description.is_empty()
                && description.len() <= MAX_DESCRIPTION
                && description
                    .chars()
                    .all(|character| character.is_alphanumeric()
                        || "_!@#-.?, \t".contains(character)),
            "case {case}: expected a connectorDescription within ConnectorDescription's \
             length and pattern, got {description:?}"
        );
        descriptions.push(description);
        bodies.push((case.to_string(), reported));
    }

    // A description belongs to ONE connector: no two types may answer the same one, which
    // a configuration shared by every type could not satisfy.
    for (left, (case, _)) in bodies.iter().enumerate() {
        for (right, (other, _)) in bodies.iter().enumerate() {
            if left >= right {
                continue;
            }
            assert_ne!(
                descriptions[left], descriptions[right],
                "case {case} and {other}: two connector types reported the same \
                 connectorDescription {:?}",
                descriptions[left]
            );
            assert_ne!(
                bodies[left].1, bodies[right].1,
                "case {case} and {other}: two connector types reported the same \
                 configuration {:?}",
                bodies[left].1
            );
        }
    }

    // And the description of a connector is the name AppFlow publishes for it.
    for (case, expected) in DESCRIPTIONS {
        let reported = bodies
            .iter()
            .find(|(connector_type, _)| connector_type == case)
            .map(|(_, body)| body.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            reported.get("connectorDescription").and_then(Value::as_str),
            Some(*expected),
            "case {case}: expected connectorDescription {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn only_a_connector_that_declared_its_capabilities_reports_them() {
    let (router, engine) = appflow_with_engine();

    // One registration per data-transfer class a connector can declare, so the reported
    // value can only have come from the record rather than from a shared default.
    let registrations: &[(&str, &[&str])] = &[
        ("AcmeCrm", &["RECORD"]),
        ("BetaWarehouse", &["FILE"]),
        ("Gamma.Archive-9", &["RECORD", "FILE"]),
    ];
    {
        let state = service_state(&*engine).expect("the AppFlow store is on this engine");
        let mut store = state.lock();
        for (label, data_transfer_types) in registrations {
            store
                .registered_connectors
                .insert(label.to_string(), registered(label, data_transfer_types));
        }
    }

    // A connector AppFlow vends registered nothing, so it declares nothing: over the
    // WHOLE catalog, none of those members is reported.
    for connector in VENDED_CONNECTORS.iter() {
        let case = connector.connector_type;
        let reported = describe(&router, case).await;
        for declared in DECLARED_MEMBERS {
            assert!(
                reported.get(declared).is_none(),
                "case {case}: a connector AppFlow vends declares no {declared}, got {reported:?}"
            );
        }
    }

    // The same operation, a registered record: every member it declared comes back, and
    // the data transfer types are the ones THAT registration declared.
    for (label, data_transfer_types) in registrations {
        let answer = call(
            &router,
            DESCRIBE_CONNECTOR,
            &json!({ "connectorType": CONNECTOR_TYPE_CUSTOM, "connectorLabel": label }),
        )
        .await;
        assert!(
            answer.is_ok(),
            "case {label}: expected HTTP 200, got {answer:?}"
        );
        let reported = answer.get("connectorConfiguration").clone();
        for declared in DECLARED_MEMBERS {
            assert!(
                reported.get(declared).is_some(),
                "case {label}: a registered connector declares {declared}, got {reported:?}"
            );
        }
        assert_eq!(
            list(&reported, "supportedDataTransferTypes"),
            data_transfer_types
                .iter()
                .map(|kind| (*kind).to_string())
                .collect::<Vec<String>>(),
            "case {label}: supportedDataTransferTypes is {:?}",
            list(&reported, "supportedDataTransferTypes")
        );
    }
}

#[tokio::test]
async fn a_member_that_does_not_apply_to_a_connector_is_unset() {
    let router = appflow();

    // Over the whole catalog, which spans both classes: a flow is driven by the connector
    // it reads FROM, so how a flow runs is reported by a source and by nothing else —
    // and an answer that does not apply is absent, not an empty list.
    for connector in VENDED_CONNECTORS.iter() {
        let case = connector.connector_type;
        let reported = describe(&router, case).await;
        for member in SOURCE_MEMBERS {
            assert_eq!(
                reported.get(member).is_some(),
                connector.can_use_as_source,
                "case {case}: canUseAsSource is {}, but {member} is {:?}",
                connector.can_use_as_source,
                reported.get(member)
            );
            assert!(
                reported
                    .get(member)
                    .is_none_or(|value| value.as_array().is_some_and(|items| !items.is_empty())),
                "case {case}: {member} is reported empty, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_destinations_a_source_offers_are_the_connectors_a_flow_can_write_to() {
    let router = appflow();

    // The ordered list the answer carries and the per-connector records it has to agree
    // with are stated separately, so first: they agree.
    assert!(
        destination_catalog_agrees(),
        "the destinations offered {DESTINATION_CONNECTORS:?} do not match the connectors \
         whose own record says a flow can write to them"
    );

    let expected: Vec<String> = DESTINATION_CONNECTORS
        .iter()
        .map(|connector_type| (*connector_type).to_string())
        .collect();
    assert!(
        expected.len() <= MAX_CONNECTOR_LIST,
        "ConnectorTypeList holds at most {MAX_CONNECTOR_LIST} entries, got {}",
        expected.len()
    );
    // A flow can write to a connector an account registered, so that type is offered too.
    assert!(
        expected.iter().any(|offered| offered == CONNECTOR_TYPE_CUSTOM),
        "expected the custom-connector type among the destinations, got {expected:?}"
    );
    for offered in &expected {
        assert!(
            CONNECTOR_TYPES.contains(&offered.as_str()),
            "expected every offered destination to be a ConnectorType the model declares, \
             got {offered:?}"
        );
    }

    // Every connector a flow can read from offers them, and in the same order: the
    // destinations are a property of the service, not of the connector being read.
    for connector in VENDED_CONNECTORS.iter().filter(|c| c.can_use_as_source) {
        let case = connector.connector_type;
        let reported = describe(&router, case).await;
        assert_eq!(
            list(&reported, "supportedDestinationConnectors"),
            expected,
            "case {case}: supportedDestinationConnectors is {:?}",
            list(&reported, "supportedDestinationConnectors")
        );
    }

    // And a destination a source offers reports that a flow can write to it, so discovery
    // cannot name a connector the service would refuse as a destination.
    for offered in expected
        .iter()
        .filter(|offered| *offered != CONNECTOR_TYPE_CUSTOM)
    {
        let reported = describe(&router, offered).await;
        assert_eq!(
            reported.get("canUseAsDestination").and_then(Value::as_bool),
            Some(true),
            "case {offered}: offered as a destination, but its own configuration is {reported:?}"
        );
    }
}
