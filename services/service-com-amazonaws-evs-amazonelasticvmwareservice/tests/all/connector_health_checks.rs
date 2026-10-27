// SPDX-License-Identifier: CC0-1.0

//! `Connector$checks` and the `Connector$status` roll-up, end to end over awsJson1_0.
//!
//! THE RULE under test: a connector's connectivity health is DERIVED from its own stored
//! record, and the checks it reports are the ones whose subject is THAT connector. A
//! connector whose connection attempt has concluded reports one `ConnectorCheck` — the
//! reachability check of the appliance type it connects to — carrying a `result` the
//! connector's `ConnectorState` decides, a `lastCheckAttempt` stamped at the instant that
//! attempt concluded, and an `impairedSince` exactly when that result is `FAILED`; a
//! connector whose attempt has not concluded reports no check at all. `status` is the
//! ROLL-UP of those results: `FAILED` when any check failed, otherwise `PASSED` when any
//! check passed, and `UNKNOWN` when no check decided anything.
//!
//! `CONNECTOR_HEALTH` is NOT one of them: the model documents it as "Aggregate health
//! across all connectors in the environment", so its subject is the environment and it is
//! reported once per environment (`tests/all/environment_connector_health.rs` owns that
//! rule). This suite pins that no connector carries it as a result of its own.
//!
//! Where each clause comes from (`smithy_query(query="shape",
//! shape="Connector,ConnectorCheck,CheckType,CheckResult,ConnectorState,ConnectorType")`):
//!
//! * "A list of checks that are run on the connector" (`Connector$checks`), each "A check
//!   on a connector to identify connectivity health" (`ConnectorCheck`).
//! * "A connector establishes a connection to the given appliance type using the
//!   credentials stored in Amazon Web Services Secrets Manager" (`Connector`) — so the
//!   `ConnectorState` of the connector IS this service's record of whether that
//!   connection stands, and "Amazon EVS requires an active connector to SDDC Manager or
//!   VCF Operations Manager to monitor environment health and license compliance"
//!   (`CreateEnvironmentConnector`) is why a caller polls it.
//! * `CheckType` declares one reachability type per `ConnectorType` —
//!   `VCENTER_REACHABILITY`, `SDDC_MANAGER_REACHABILITY`,
//!   `OPERATIONS_MANAGER_REACHABILITY` — beside the environment-scoped
//!   `CONNECTOR_HEALTH`.
//! * "The date and time of the last check attempt" (`ConnectorCheck$lastCheckAttempt`)
//!   and "The time when connector health began to be impaired"
//!   (`ConnectorCheck$impairedSince`).
//! * `Connector$status` is a `CheckResult`, whose values are `PASSED | FAILED | UNKNOWN`.
//!
//! The rows of the tables below differ along what the rule generalises over: the
//! `ConnectorType` the connector connects to (the whole modelled catalog, so the
//! reachability check type varies with it), the form of the appliance FQDN and of the
//! secret the record carries (which the health does NOT depend on), the `ConnectorState`
//! the record is in (every value the modelled enum declares), and the operation the
//! record is read back through.
//!
//! The harness (`emulator`, `call`, `create_settled`, `settle`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, create_settled, emulator, settle, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs::ops::types::CONNECTOR_TYPE_VALUES;

/// Every `ConnectorState` the model declares, from that same generated catalog
/// (`smithy_query(query="shape", shape="ConnectorState")`).
const CONNECTOR_STATES: &[&str] = evs::ops::types::CONNECTOR_STATE_VALUES;

/// Every `CheckType` the model declares, used here to pin the check types this suite
/// names as being values the model really declares.
const CHECK_TYPES: &[&str] = evs::ops::types::CHECK_TYPE_VALUES;

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// A secret given as a bare name, the other form it accepts.
const SECRET_NAME: &str = "evs/vcf-appliance-credentials";

/// The check type whose subject is the ENVIRONMENT and not one connector, written out
/// here from the model's own `Check$type` documentation (`smithy_query(query="shape",
/// shape="Check")`: "<code>CONNECTOR_HEALTH</code>: Aggregate health across all connectors
/// in the environment"), so this suite can pin that no connector reports it.
pub(crate) const CONNECTOR_HEALTH: &str = "CONNECTOR_HEALTH";

/// The reachability check type of the appliance a connector of `connector_type` connects
/// to — written out here from the model's own `CheckType` enumeration
/// (`smithy_query(query="shape", shape="CheckType")`), so this suite states the mapping
/// instead of reading it back out of the code under test.
fn reachability_check_type(connector_type: &str) -> &'static str {
    match connector_type {
        "VCENTER" => "VCENTER_REACHABILITY",
        "SDDC_MANAGER" => "SDDC_MANAGER_REACHABILITY",
        "OPERATIONS_MANAGER" => "OPERATIONS_MANAGER_REACHABILITY",
        other => panic!("the model declares no reachability check type for {other:?}"),
    }
}

/// The check types a connector of `connector_type` reports once its record has decided:
/// the reachability check of the appliance it connects to, and nothing else.
fn expected_check_types(connector_type: &str) -> Vec<&'static str> {
    vec![reachability_check_type(connector_type)]
}

/// An appliance FQDN for a connector of `connector_type`, of the modelled `ApplianceFqdn`
/// form.
fn fqdn_for(connector_type: &str) -> String {
    format!(
        "{}.evs.example.com",
        connector_type.to_ascii_lowercase().replace('_', "-")
    )
}

/// Create one connector in `environment_id` and return the record the create reported.
pub(crate) async fn create_connector(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_type: &str,
    appliance_fqdn: &str,
    secret_identifier: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreateEnvironmentConnector",
        &json!({
            "environmentId": environment_id,
            "type": connector_type,
            "applianceFqdn": appliance_fqdn,
            "secretIdentifier": secret_identifier,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a {connector_type} connector expected 200, got {status} {body}"
    );
    body["connector"].clone()
}

/// The record `ListEnvironmentConnectors` reports for one connector of one environment.
/// The suites here hold one connector per environment, so a `nextToken` would be a defect
/// of the list rather than a page to follow.
async fn listed_connector(
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
        "case {case:?}: the list expected 200, got {status} {body}"
    );
    let listed = body["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
        .clone();
    listed
        .iter()
        .find(|connector| connector["connectorId"] == json!(connector_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the list reports no connector {connector_id}: {listed:?}")
        })
        .clone()
}

/// The `connectorId` of a reported connector.
fn connector_id_of(case: &str, connector: &Value) -> String {
    connector["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {connector}"))
        .to_string()
}

/// Assert one member of a reported connector.
fn assert_member(case: &str, reported: &Value, member: &str, expected: &Value) {
    assert_eq!(
        &reported[member], expected,
        "case {case:?}: {member}: expected {expected}, got {}",
        reported[member]
    );
}

/// Force one stored connector to `state`, so the health a record in that state decides is
/// exercised whatever operation could put it there: no operation of this service stages a
/// failed connection, so `CREATE_FAILED` and `UPDATE_FAILED` are reachable no other way.
pub(crate) fn force_connector_state(
    router: &Emulator,
    environment_id: &str,
    connector_id: &str,
    state: &str,
) {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let mut store = store.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    let connector = environment
        .connectors
        .get_mut(connector_id)
        .unwrap_or_else(|| panic!("the store holds the connector {connector_id}"));
    connector.state = state.to_string();
    connector.pending = None;
}

/// Assert that a connector reports NO check and rolls up to `UNKNOWN` — what a record
/// whose connection attempt has not concluded decides.
fn assert_undecided(case: &str, connector: &Value) {
    assert!(
        connector["checks"].is_null(),
        "case {case:?}: a connector whose attempt has not concluded reports no check, got {}",
        connector["checks"]
    );
    assert_member(case, connector, "status", &json!("UNKNOWN"));
}

/// Assert the checks a connector of `connector_type` reports all carry `result`, are the
/// check types the model declares for it, are stamped at the connector's own `modifiedAt`,
/// and carry an `impairedSince` exactly when the result is `FAILED`. `status` must roll
/// those results up to `result`.
fn assert_decided(case: &str, connector_type: &str, result: &str, connector: &Value) {
    let checks = connector["checks"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a connector whose attempt concluded reports checks, got {connector}")
        })
        .clone();
    let reported_types: Vec<&str> = checks
        .iter()
        .map(|check| check["type"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        reported_types,
        expected_check_types(connector_type),
        "case {case:?}: expected the connector-scoped check types of a {connector_type} connector, got {reported_types:?}"
    );
    for check in &checks {
        let check_type = check["type"].as_str().unwrap_or("<none>");
        assert!(
            CHECK_TYPES.contains(&check_type),
            "case {case:?}: {check_type:?} is no value of the modelled CheckType enum"
        );
        assert_ne!(
            check_type, CONNECTOR_HEALTH,
            "case {case:?}: {CONNECTOR_HEALTH} aggregates the environment's connectors, so \
             no single connector reports it: {check}"
        );
        assert_eq!(
            check["result"],
            json!(result),
            "case {case:?}: {check_type}: expected result {result}, got {check}"
        );
        assert_eq!(
            check["lastCheckAttempt"], connector["modifiedAt"],
            "case {case:?}: {check_type}: the attempt is stamped at the instant the record settled ({}), got {check}",
            connector["modifiedAt"]
        );
        if result == "FAILED" {
            assert_eq!(
                check["impairedSince"], connector["modifiedAt"],
                "case {case:?}: {check_type}: a failed check is impaired since the record failed ({}), got {check}",
                connector["modifiedAt"]
            );
        } else {
            assert!(
                check["impairedSince"].is_null(),
                "case {case:?}: {check_type}: only a failed check is impaired since anything, got {check}"
            );
        }
    }
    assert_member(case, connector, "status", &json!(result));
}

#[tokio::test]
async fn an_active_connector_reports_the_connectivity_health_its_record_decides() {
    let router = emulator();

    // Each row: the case, the connector type and the secret identifier the record
    // carries. The rows cover the whole modelled `ConnectorType` catalog — the dimension
    // the reachability check type varies along — in both forms of `secretIdentifier`, a
    // fact the connectivity health does not depend on.
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a vCenter connector with an ARN secret",
            "VCENTER",
            SECRET_ARN,
        ),
        (
            "an SDDC Manager connector with a named secret",
            "SDDC_MANAGER",
            SECRET_NAME,
        ),
        (
            "an Operations Manager connector with an ARN secret",
            "OPERATIONS_MANAGER",
            SECRET_ARN,
        ),
        (
            "a vCenter connector with a named secret",
            "VCENTER",
            SECRET_NAME,
        ),
    ];
    // Every modelled type is covered by the rows above, so the catalog and the table
    // cannot drift apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter().any(|(_, row_type, _)| row_type == connector_type),
            "the table must cover the modelled type {connector_type:?}"
        );
    }

    for (case, connector_type, secret_identifier) in rows {
        // A fresh environment per row, so each row reads back only its own record.
        let environment = create_settled(&router, None).await;
        let fqdn = fqdn_for(connector_type);
        let created = create_connector(
            &router,
            case,
            &environment,
            connector_type,
            &fqdn,
            secret_identifier,
        )
        .await;
        let connector_id = connector_id_of(case, &created);

        // The connection is still being established, so nothing has concluded yet.
        assert_member(case, &created, "state", &json!("CREATING"));
        assert_undecided(case, &created);

        // Once the connector settles into ACTIVE the connection to the appliance stands,
        // so both of its checks are decided and the roll-up is PASSED.
        settle(&router);
        let active = listed_connector(&router, case, &environment, &connector_id).await;
        assert_member(case, &active, "state", &json!("ACTIVE"));
        assert_decided(case, connector_type, "PASSED", &active);

        // A read changes nothing a later caller sees: the same checks and roll-up.
        let again = listed_connector(&router, case, &environment, &connector_id).await;
        assert_eq!(
            again, active,
            "case {case:?}: a second list reported a different record: {active} then {again}"
        );

        // Reconfiguring the connector puts the connection back in flight, so the health
        // is undecided again until the change settles — and the new attempt is stamped at
        // the instant it concluded, which is later than the first one's.
        let (status, body) = call(
            &router,
            "UpdateEnvironmentConnector",
            &json!({
                "environmentId": environment,
                "connectorId": connector_id,
                "applianceFqdn": format!("replacement.{fqdn}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the update expected 200, got {status} {body}"
        );
        let updating = body["connector"].clone();
        assert_member(case, &updating, "state", &json!("UPDATING"));
        assert_undecided(case, &updating);

        settle(&router);
        let settled = listed_connector(&router, case, &environment, &connector_id).await;
        assert_member(case, &settled, "state", &json!("ACTIVE"));
        assert_decided(case, connector_type, "PASSED", &settled);
        let (before, after) = (
            active["modifiedAt"].as_f64().unwrap_or_default(),
            settled["modifiedAt"].as_f64().unwrap_or_default(),
        );
        assert!(
            after >= before,
            "case {case:?}: the reconfigured connector settled before it was updated: {before} then {after}"
        );
    }
}

#[tokio::test]
async fn the_connector_state_decides_what_its_checks_report() {
    let router = emulator();

    // Each row: the `ConnectorState` the record is in, the connector type it carries, and
    // the result its checks must report — `None` when the record has decided nothing, so
    // no check is reported and the roll-up is UNKNOWN. Every value of the modelled
    // `ConnectorState` enum is a row, and the type is cycled across them so the two
    // dimensions are crossed.
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        // The connection is still being established.
        ("CREATING", "VCENTER", None),
        // It stands: the appliance answered the connector.
        ("ACTIVE", "SDDC_MANAGER", Some("PASSED")),
        // Reconfiguration put it back in flight.
        ("UPDATING", "OPERATIONS_MANAGER", None),
        // It is being torn down, and then is gone.
        ("DELETING", "VCENTER", None),
        ("DELETED", "SDDC_MANAGER", None),
        // Establishing it CONCLUDED without a connection: connectivity health failed.
        ("CREATE_FAILED", "OPERATIONS_MANAGER", Some("FAILED")),
        ("UPDATE_FAILED", "VCENTER", Some("FAILED")),
    ];
    for state in CONNECTOR_STATES {
        assert!(
            rows.iter().any(|(row_state, _, _)| row_state == state),
            "the table must cover the modelled state {state:?}"
        );
    }

    for (state, connector_type, expected) in rows {
        let case = format!("a {connector_type} connector in {state}");
        let case = case.as_str();
        let environment = create_settled(&router, None).await;
        let fqdn = fqdn_for(connector_type);
        let created = create_connector(
            &router,
            case,
            &environment,
            connector_type,
            &fqdn,
            SECRET_ARN,
        )
        .await;
        let connector_id = connector_id_of(case, &created);

        // Drive the record into the state under test the way a client reaches it, and
        // write the two states no operation of this service produces straight into the
        // store.
        match state {
            "CREATING" => {}
            "ACTIVE" => settle(&router),
            "UPDATING" => {
                settle(&router);
                let (status, body) = call(
                    &router,
                    "UpdateEnvironmentConnector",
                    &json!({
                        "environmentId": environment,
                        "connectorId": connector_id,
                        "secretIdentifier": SECRET_NAME,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the update expected 200, got {status} {body}"
                );
            }
            "DELETING" | "DELETED" => {
                settle(&router);
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
                if state == "DELETED" {
                    settle(&router);
                }
            }
            "CREATE_FAILED" => force_connector_state(&router, &environment, &connector_id, state),
            "UPDATE_FAILED" => {
                settle(&router);
                force_connector_state(&router, &environment, &connector_id, state);
            }
            other => panic!("case {case:?}: the table drove no record into {other:?}"),
        }

        let reported = listed_connector(&router, case, &environment, &connector_id).await;
        assert_member(case, &reported, "state", &json!(state));
        match expected {
            Some(result) => assert_decided(case, connector_type, result, &reported),
            None => assert_undecided(case, &reported),
        }
    }
}
