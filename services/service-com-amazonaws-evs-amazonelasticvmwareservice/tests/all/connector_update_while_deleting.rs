// SPDX-License-Identifier: CC0-1.0

//! Reconfiguring a connector whose teardown has already started, end to end over
//! awsJson1_0.
//!
//! The rule under test: `UpdateEnvironmentConnector` resolves only a connector the
//! service can still act on, so a connector whose deletion has already been accepted —
//! whether by `DeleteEnvironmentConnector` or by the deletion of the environment that
//! holds it — resolves to nothing, whatever the update asks for. The request is refused
//! with the operation's modelled `ResourceNotFoundException` naming the connector the
//! caller asked for ("The resource might not be specified correctly, or it may have a
//! `state` of `DELETED`", `ResourceNotFoundException` doc), nothing about the record
//! changes, and the deletion the service already owed still comes due — so the type the
//! connector occupied really does become free for a new connector ("You can create only
//! one connector of each type per environment", `CreateEnvironmentConnector` doc).
//!
//! The harness (`emulator`, `call`, `create_settled`, `settle`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{
    call, create_settled, emulator, error_type, secret_arn_of, settle, Emulator,
};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs::ops::types::CONNECTOR_TYPE_VALUES;

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// A secret given as a bare name, the other form it accepts.
const SECRET_NAME: &str = "evs/vcf-appliance-credentials";

/// The `resourceType` the not-found error names for a connector.
const RESOURCE_TYPE_CONNECTOR: &str = "connector";

/// An FQDN at the modelled `ApplianceFqdn` maximum of 253 characters: three 63-byte
/// labels (the longest a DNS label can be under that member's own pattern) and a
/// 61-byte one, dot-separated.
fn longest_fqdn() -> String {
    let label = "a".repeat(63);
    format!("{label}.{label}.{label}.{}", "a".repeat(61))
}

/// Create an environment with one connector of `connector_type` and return the
/// environment id together with the `connectorId` the create minted.
async fn environment_with_connector(
    router: &Emulator,
    case: &str,
    connector_type: &str,
    appliance_fqdn: &str,
) -> (String, String) {
    // Settled: a list addressed to an environment that is still initializing is refused
    // (`tests/all/list_while_initializing.rs`), and every test here reads the connector
    // back through `ListEnvironmentConnectors`.
    let environment = create_settled(router, None).await;
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment,
            "type": connector_type,
            "applianceFqdn": appliance_fqdn,
            "secretIdentifier": SECRET_ARN,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create expected 200, got {status} {body}"
    );
    let connector_id = body["connector"]["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {body}"))
        .to_string();
    // Let the create settle, so any later state is the delete's or the update's doing.
    settle(router);
    (environment, connector_id)
}

/// The record `ListEnvironmentConnectors` reports for one connector of an environment.
async fn stored_connector(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list expected 200, got {status} {body}"
    );
    body["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
        .iter()
        .find(|connector| connector["connectorId"] == json!(connector_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the list reports no connector {connector_id}: {body}")
        })
        .clone()
}

/// Assert that `body` is the `ResourceNotFoundException` this service answers for
/// `connector_id`, reported with the awsJson front end's HTTP 400 (the body's `__type`
/// is what tells one modelled client error apart; see `tests/all/error_wire_status.rs`).
fn assert_connector_not_found(case: &str, status: u16, body: &Value, connector_id: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body).rsplit(['#', '/']).next().unwrap_or(""),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {}",
        error_type(body)
    );
    assert_eq!(
        body["resourceId"],
        json!(connector_id),
        "case {case:?}: resourceId must name the requested connector, got {body}"
    );
    assert_eq!(
        body["resourceType"],
        json!(RESOURCE_TYPE_CONNECTOR),
        "case {case:?}: unexpected resourceType, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(format!("Connector {connector_id} was not found")),
        "case {case:?}: the message must name the requested connector, got {body}"
    );
}

#[tokio::test]
async fn an_update_of_a_connector_whose_deletion_has_started_is_refused_and_writes_nothing() {
    let router = emulator();

    // Each row: the case, the connector type, the appliance FQDN it was created with,
    // and the member the update names with its new value. The rows differ along what the
    // rule generalises over — the connector type (the whole modelled catalog), which of
    // the two reconfigurable members the update asks for, and the form and length of the
    // new value (a multi-label FQDN, a single-label one, one at the modelled 253-
    // character maximum, a secret as an ARN and a secret as a bare name).
    let rows: Vec<(&str, &str, String, &str, String)> = vec![
        (
            "a vCenter connector, a new multi-label FQDN",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            "applianceFqdn",
            "vcenter-2.evs.example.com".to_string(),
        ),
        (
            "a vCenter connector, a new single-label FQDN",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            "applianceFqdn",
            "vcenter2".to_string(),
        ),
        (
            "an SDDC Manager connector, a new FQDN at the modelled 253-character maximum",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            "applianceFqdn",
            longest_fqdn(),
        ),
        (
            "an Operations Manager connector, a new secret given as an ARN",
            "OPERATIONS_MANAGER",
            "operations-manager".to_string(),
            "secretIdentifier",
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/rotated-ZyXw98".to_string(),
        ),
        (
            "an SDDC Manager connector, a new secret given as a bare name",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            "secretIdentifier",
            "evs/rotated-credentials".to_string(),
        ),
    ];
    // Every modelled type is covered by the rows above, so the catalog and the table
    // cannot drift apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter().any(|(_, row_type, ..)| row_type == connector_type),
            "the table must cover the modelled type {connector_type:?}"
        );
    }

    for (case, connector_type, appliance_fqdn, member, new_value) in rows {
        let (environment, connector_id) =
            environment_with_connector(&router, case, connector_type, &appliance_fqdn).await;

        // The teardown the caller accepts: the connector is DELETING and the clock owes
        // it DELETED.
        let (status, body) = call(
            &router,
            "DeleteEnvironmentConnector",
            &json!({ "environmentId": environment, "connectorId": connector_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {body}"
        );
        assert_eq!(
            body["connector"]["state"],
            json!("DELETING"),
            "case {case:?}: the delete must report a DELETING connector, got {body}"
        );
        let deleting = stored_connector(&router, case, &environment, &connector_id).await;

        // The update, while the teardown is still in flight: the connector resolves to
        // nothing.
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                member: new_value,
            }),
        )
        .await;
        assert_connector_not_found(case, status, &body, &connector_id);

        // Nothing was written: the record is exactly the one the delete left, so neither
        // the new metadata nor a state or stamp of its own reached the store.
        let after = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            after, deleting,
            "case {case:?}: a refused update must write nothing, got {after}"
        );
        let reconfigured = if member == "applianceFqdn" {
            json!(new_value)
        } else {
            json!(secret_arn_of(&new_value))
        };
        let stored_member = if member == "applianceFqdn" {
            "applianceFqdn"
        } else {
            "secretArn"
        };
        assert_ne!(
            after[stored_member], reconfigured,
            "case {case:?}: {stored_member} must not take the refused value, got {after}"
        );

        // The deletion the service already owed still comes due — the refused update did
        // not throw it away.
        settle(&router);
        let settled = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            settled["state"],
            json!("DELETED"),
            "case {case:?}: the accepted deletion must still reach DELETED, got {settled}"
        );

        // A connector that has reached DELETED is refused the same way.
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                member: new_value,
            }),
        )
        .await;
        assert_connector_not_found(case, status, &body, &connector_id);

        // And the type the deleted connector occupied is free for a new connector, which
        // only a record that really reached DELETED allows.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "type": connector_type,
                "applianceFqdn": appliance_fqdn,
                "secretIdentifier": SECRET_NAME,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replacement connector expected 200, got {status} {body}"
        );
        assert_ne!(
            body["connector"]["connectorId"],
            json!(connector_id),
            "case {case:?}: the replacement must be a new record, got {body}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_connector_of_an_environment_being_deleted_is_refused() {
    let router = emulator();

    // Each row: the case and the member the update names with its new value. Deleting
    // the environment starts the deletion of the connectors that belong to it, so those
    // records are no longer connectors any update can act on either.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a new FQDN while the environment is being deleted",
            "applianceFqdn",
            "vcenter-2.evs.example.com".to_string(),
        ),
        (
            "a new secret while the environment is being deleted",
            "secretIdentifier",
            SECRET_NAME.to_string(),
        ),
    ];

    for (case, member, new_value) in rows {
        let (environment, connector_id) =
            environment_with_connector(&router, case, "VCENTER", "vcenter.evs.example.com").await;

        let (status, body) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the environment delete expected 200, got {status} {body}"
        );
        let deleting = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            deleting["state"],
            json!("DELETING"),
            "case {case:?}: the environment's deletion must start the connector's, got {deleting}"
        );

        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                member: new_value,
            }),
        )
        .await;
        assert_connector_not_found(case, status, &body, &connector_id);

        let after = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            after, deleting,
            "case {case:?}: a refused update must write nothing, got {after}"
        );
    }
}
