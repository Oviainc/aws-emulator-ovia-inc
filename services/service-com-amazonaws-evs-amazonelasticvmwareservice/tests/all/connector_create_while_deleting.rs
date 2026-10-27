// SPDX-License-Identifier: CC0-1.0

//! Creating a connector of a type whose connector is still being torn down, end to end
//! over awsJson1_0.
//!
//! The rule under test: the record `CreateEnvironmentConnector` reports is the connector
//! the caller is handed as the created one, so a repeat create reports a stored record
//! only while that record is still one a later call resolves. A connector whose deletion
//! this service has already accepted resolves to nothing ("The resource might not be
//! specified correctly, or it may have a `state` of `DELETED`",
//! `ResourceNotFoundException` doc) while still occupying its type ("You can create only
//! one connector of each type per environment", `CreateEnvironmentConnector` doc), so a
//! create for that type inside the teardown window is refused with the operation's
//! modelled `ValidationException` — whatever appliance and secret it names, and whether
//! the teardown was started by `DeleteEnvironmentConnector` or by the deletion of the
//! environment that holds the connector. Nothing is stored for the refused create, and
//! once the teardown comes due the type really is free: the create then mints its own
//! `CREATING` record.
//!
//! The other side of the same rule: a connector in any state the service can still act
//! on — `CREATING`, `ACTIVE`, `UPDATING` or a state that concluded without a connection —
//! is reported by the idempotent repeat, with HTTP 200 and no second record.
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

/// An FQDN at the modelled `ApplianceFqdn` maximum of 253 characters: three 63-byte
/// labels (the longest a DNS label can be under that member's own pattern) and a 61-byte
/// one, dot-separated.
fn longest_fqdn() -> String {
    let label = "a".repeat(63);
    format!("{label}.{label}.{label}.{}", "a".repeat(61))
}

/// A `CreateEnvironmentConnector` request.
fn create_request(
    environment_id: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
) -> Value {
    json!({
        "environmentId": environment_id,
        "type": connector_type,
        "applianceFqdn": appliance_fqdn,
        "secretIdentifier": secret_identifier,
    })
}

/// Create an environment with one connector and return the environment id together with
/// the `connectorId` the create minted. `settle_create` applies the transition the clock
/// owes the new connector, so the record is `ACTIVE` rather than `CREATING` when the
/// caller goes on.
async fn environment_with_connector(
    router: &Emulator,
    case: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
    settle_create: bool,
) -> (String, String) {
    // Settled: a list addressed to an environment that is still initializing is refused
    // (`tests/all/list_while_initializing.rs`), and every test here reads the connectors
    // back through `ListEnvironmentConnectors`.
    let environment = create_settled(router, None).await;
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &create_request(&environment, connector_type, appliance_fqdn, secret_identifier),
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
    if settle_create {
        settle(router);
    }
    (environment, connector_id)
}

/// Every connector record one environment holds, as `ListEnvironmentConnectors` reports
/// them. The suites here hold far fewer than one page, so a `nextToken` would be a defect
/// of the list rather than a page to follow.
async fn listed_connectors(router: &Emulator, case: &str, environment_id: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the list expected 200, got {status} {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: the whole population fits one page, got {body}"
    );
    body["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
        .clone()
}

/// The record `ListEnvironmentConnectors` reports for one connector of an environment.
async fn stored_connector(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
) -> Value {
    listed_connectors(router, case, environment_id)
        .await
        .into_iter()
        .find(|connector| connector["connectorId"] == json!(connector_id))
        .unwrap_or_else(|| panic!("case {case:?}: the list reports no connector {connector_id}"))
}

/// Assert that `body` is the `ValidationException` this service answers a create whose
/// connector type is already taken, reported with the awsJson front end's HTTP 400 (the
/// body's `__type` is what tells one modelled client error apart; see
/// `tests/all/error_wire_status.rs`).
fn assert_type_is_taken(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body).rsplit(['#', '/']).next().unwrap_or(""),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        error_type(body)
    );
    assert_eq!(
        body["fieldList"][0]["name"],
        json!("type"),
        "case {case:?}: the refusal must name the `type` member, got {body}"
    );
}

#[tokio::test]
async fn a_create_for_a_type_whose_connector_is_still_being_deleted_is_refused() {
    let router = emulator();

    // Each row: the case, the connector type, the appliance FQDN and secret the stored
    // connector carries, and the appliance FQDN and secret the create made inside the
    // teardown window names. The rows differ along what the rule generalises over — the
    // connector type (the whole modelled catalog), whether the second create repeats the
    // stored metadata exactly or names different metadata, and the form and length of
    // each value (an ARN secret, a bare-name secret, a multi-label FQDN, a single-label
    // one, and one at the modelled `ApplianceFqdn` 253-character maximum). The row that
    // repeats the metadata exactly is the one a repeat create would have answered 200
    // for.
    let rows: Vec<(&str, &str, String, &str, String, &str)> = vec![
        (
            "a vCenter connector recreated with exactly the same metadata",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN,
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN,
        ),
        (
            "an SDDC Manager connector recreated with the same metadata, named secret",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            SECRET_NAME,
            "sddc-manager.evs.example.com".to_string(),
            SECRET_NAME,
        ),
        (
            "an Operations Manager connector recreated with the same single-label FQDN",
            "OPERATIONS_MANAGER",
            "operations-manager".to_string(),
            SECRET_ARN,
            "operations-manager".to_string(),
            SECRET_ARN,
        ),
        (
            "a vCenter connector recreated at the modelled 253-character FQDN maximum",
            "VCENTER",
            longest_fqdn(),
            SECRET_NAME,
            longest_fqdn(),
            SECRET_NAME,
        ),
        (
            "a vCenter connector recreated for another appliance",
            "VCENTER",
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN,
            "vcenter-2.evs.example.com".to_string(),
            SECRET_ARN,
        ),
        (
            "an SDDC Manager connector recreated with another secret",
            "SDDC_MANAGER",
            "sddc-manager.evs.example.com".to_string(),
            SECRET_ARN,
            "sddc-manager.evs.example.com".to_string(),
            SECRET_NAME,
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

    for (case, connector_type, stored_fqdn, stored_secret, wanted_fqdn, wanted_secret) in rows {
        let (environment, connector_id) = environment_with_connector(
            &router,
            case,
            connector_type,
            &stored_fqdn,
            stored_secret,
            // Settled, so any later state is the delete's doing.
            true,
        )
        .await;

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

        // The create, while the teardown is still in flight.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(&environment, connector_type, &wanted_fqdn, wanted_secret),
        )
        .await;
        assert_type_is_taken(case, status, &body);
        assert!(
            body["connector"].is_null(),
            "case {case:?}: a refused create reports no connector, got {body}"
        );

        // Nothing was stored for it: the environment still holds exactly the one record
        // the delete left, untouched.
        let held = listed_connectors(&router, case, &environment).await;
        assert_eq!(
            held,
            vec![deleting.clone()],
            "case {case:?}: a refused create must store nothing, got {held:?}"
        );

        // The deletion the service already owed still comes due — the refused create did
        // not throw it away.
        settle(&router);
        let settled = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            settled["state"],
            json!("DELETED"),
            "case {case:?}: the accepted deletion must still reach DELETED, got {settled}"
        );

        // And now the type is free: the create mints its OWN record, which is what the
        // caller asked for both times.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(&environment, connector_type, &wanted_fqdn, wanted_secret),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replacement connector expected 200, got {status} {body}"
        );
        let replacement = body["connector"].clone();
        assert_ne!(
            replacement["connectorId"],
            json!(connector_id),
            "case {case:?}: the replacement must be its own record, got {replacement}"
        );
        assert_eq!(
            replacement["state"],
            json!("CREATING"),
            "case {case:?}: a minted connector is CREATING, got {replacement}"
        );
        assert_eq!(
            replacement["applianceFqdn"],
            json!(wanted_fqdn),
            "case {case:?}: the replacement must carry the requested appliance, got {replacement}"
        );
        assert_eq!(
            replacement["secretArn"],
            json!(secret_arn_of(wanted_secret)),
            "case {case:?}: the replacement must carry the requested secret, got {replacement}"
        );
        // The read-back: the list reports the record the create persisted, beside the
        // record of the connector that was deleted.
        let listed = stored_connector(
            &router,
            case,
            &environment,
            replacement["connectorId"].as_str().unwrap_or_default(),
        )
        .await;
        assert_eq!(
            listed, replacement,
            "case {case:?}: the list must report the minted connector, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_create_for_a_type_whose_connector_the_environments_deletion_is_tearing_down_is_refused() {
    let router = emulator();

    // Each row: the case and the metadata the create made after the environment's
    // deletion names. Deleting the environment starts the deletion of the connectors that
    // belong to it, so those records are on their way out too — a second way into the
    // same teardown window.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "the stored metadata repeated while the environment is being deleted",
            "vcenter.evs.example.com".to_string(),
            SECRET_ARN,
        ),
        (
            "other metadata while the environment is being deleted",
            "vcenter-2.evs.example.com".to_string(),
            SECRET_NAME,
        ),
    ];

    for (case, wanted_fqdn, wanted_secret) in rows {
        let (environment, connector_id) = environment_with_connector(
            &router,
            case,
            "VCENTER",
            "vcenter.evs.example.com",
            SECRET_ARN,
            true,
        )
        .await;

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
            "CreateEnvironmentConnector",
            &create_request(&environment, "VCENTER", &wanted_fqdn, wanted_secret),
        )
        .await;
        assert_type_is_taken(case, status, &body);

        let held = listed_connectors(&router, case, &environment).await;
        assert_eq!(
            held,
            vec![deleting.clone()],
            "case {case:?}: a refused create must store nothing, got {held:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_still_reports_a_connector_the_service_can_act_on() {
    let router = emulator();

    // Each row: the case, the `ConnectorState` the stored connector is in when the repeat
    // arrives, and how to get it there. Every state in the modelled `ConnectorState`
    // catalog (`smithy_query(query="shape", shape="ConnectorState")`) that is not a
    // teardown state belongs here, because each of them is a connector the service can
    // still act on — so the idempotent repeat reports the stored record rather than
    // minting a second one.
    let rows: [(&str, &str); 5] = [
        ("the create has not settled yet", "CREATING"),
        ("the connection is established", "ACTIVE"),
        ("a reconfiguration is in flight", "UPDATING"),
        ("establishing the connection concluded without one", "CREATE_FAILED"),
        ("a reconfiguration concluded without one", "UPDATE_FAILED"),
    ];
    // The teardown states are the other test's subject, and every other modelled state is
    // a row here.
    for state in evs::ops::types::CONNECTOR_STATE_VALUES {
        if *state == evs::state::CONNECTOR_DELETING || *state == evs::state::CONNECTOR_DELETED {
            continue;
        }
        assert!(
            rows.iter().any(|(_, row_state)| row_state == state),
            "the table must cover the modelled state {state:?}"
        );
    }

    for (case, state) in rows {
        let (environment, connector_id) = environment_with_connector(
            &router,
            case,
            "VCENTER",
            "vcenter.evs.example.com",
            SECRET_ARN,
            // Straight out of the create for the CREATING row, which is the state the
            // transition the clock owes it has not come due for yet.
            state != "CREATING",
        )
        .await;
        match state {
            // The state the create itself left, and the state the settled create left.
            "CREATING" | "ACTIVE" => {}
            "UPDATING" => {
                let (status, body) = call(
                    &router,
                    "UpdateEnvironmentConnector",
                    &json!({
                        "environmentId": environment,
                        "connectorId": connector_id,
                        "applianceFqdn": "vcenter-2.evs.example.com",
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the update expected 200, got {status} {body}"
                );
            }
            // A state nothing in this emulator writes, so the store is what puts the
            // record in it: establishing or reconfiguring the connection concluded
            // without a connection.
            concluded => {
                let store = evs::state::service_state(&*router.engine)
                    .expect("the in-memory engine carries this service's store");
                let mut store = store.lock();
                let stored = store
                    .environments
                    .get_mut(&environment)
                    .expect("the store holds the environment under test")
                    .connectors
                    .get_mut(&connector_id)
                    .expect("the environment holds the connector under test");
                stored.state = concluded.to_string();
                stored.pending = None;
            }
        }

        let before = stored_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            before["state"],
            json!(state),
            "case {case:?}: the connector must be {state} before the repeat, got {before}"
        );

        // The repeat names the metadata the stored record carries, so it is the same
        // create again: `@idempotent`.
        let (status, body) = call(
            &router,
            "CreateEnvironmentConnector",
            &create_request(
                &environment,
                "VCENTER",
                before["applianceFqdn"].as_str().unwrap_or_default(),
                before["secretArn"].as_str().unwrap_or_default(),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat create expected 200, got {status} {body}"
        );
        assert_eq!(
            body["connector"], before,
            "case {case:?}: the repeat must report the stored record, got {body}"
        );
        let held = listed_connectors(&router, case, &environment).await;
        assert_eq!(
            held,
            vec![before.clone()],
            "case {case:?}: the repeat must mint nothing, got {held:?}"
        );
    }
}
