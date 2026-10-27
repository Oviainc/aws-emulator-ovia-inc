// SPDX-License-Identifier: CC0-1.0

//! The environment-scoped `CONNECTOR_HEALTH` check, end to end over awsJson1_0.
//!
//! THE RULE under test: a check whose documented subject is the ENVIRONMENT is reported
//! in `Environment$checks` and, where its facts are the environment's child records, its
//! `result` is the roll-up over all of them — so `CONNECTOR_HEALTH` is reported once per
//! environment, aggregating every connector that environment holds, and never as one
//! connector's own result. One impaired connector impairs the aggregate; connectors that
//! have decided nothing (and an environment holding no connector at all) aggregate to
//! `UNKNOWN`, which impairs nothing.
//!
//! Where each clause comes from (`smithy_query(query="shape",
//! shape="Check,CheckType,CheckResult,ConnectorCheck")` and the `Environment` members):
//!
//! * "<code>CONNECTOR_HEALTH</code>: Aggregate health across all connectors in the
//!   environment." (`Check$type`.)
//! * "A check on the environment to identify connector health." (`Environment$checks`.)
//! * "A check on a connector to identify connectivity health." (`ConnectorCheck`) — the
//!   connector-scoped checks, which `tests/all/connector_health_checks.rs` owns.
//! * "Reports impaired functionality that stems from issues internal to the environment."
//!   (`Environment$environmentStatus`, a `CheckResult` whose values are `PASSED |
//!   FAILED | UNKNOWN`.)
//! * "A unique ID for the check." (`Check$id`.) and "The time when environment health
//!   began to be impaired." (`Check$impairedSince`.)
//!
//! The rows of the table below differ along what the rule generalises over: how many
//! connectors the environment holds (none, one, two, every modelled `ConnectorType`),
//! which types they are, and the `ConnectorState` each is in — the connection established,
//! still in flight, concluded without a connection, being torn down, or gone. Every row
//! also cross-checks the aggregate against the per-connector statuses
//! `ListEnvironmentConnectors` reports, so the two surfaces cannot disagree.
//!
//! The harness (`emulator`, `call`, `settle`) is the one `environment_lifecycle` owns, and
//! the connector helpers are `connector_health_checks`', so every test drives the service
//! the way a client does: `POST /` with `X-Amz-Target:
//! AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};

use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types as evs_types;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::connector_health_checks::{create_connector, force_connector_state, CONNECTOR_HEALTH};
use crate::environment_lifecycle::{call, emulator, error_type, settle, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs_types::CONNECTOR_TYPE_VALUES;

/// The `CheckResult` values, written out from the model's own enumeration
/// (`smithy_query(query="shape", shape="CheckResult")`), so this suite states the roll-up
/// instead of reading it back out of the code under test.
const PASSED: &str = "PASSED";
/// See [`PASSED`].
const FAILED: &str = "FAILED";
/// See [`PASSED`].
const UNKNOWN: &str = "UNKNOWN";

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// The roll-up of a set of `CheckResult` values: impaired when any of them failed,
/// otherwise healthy when any of them passed, and `UNKNOWN` when none decided anything.
fn rolled_up(results: &[&str]) -> &'static str {
    if results.contains(&FAILED) {
        FAILED
    } else if results.contains(&PASSED) {
        PASSED
    } else {
        UNKNOWN
    }
}

/// An environment whose OTHER health checks cannot fail: it is created with the minimum
/// host count and carries no license key, so `HOST_COUNT` passes and the two license
/// checks are undecidable. `environmentStatus` therefore reports exactly what the
/// connector aggregate contributes.
async fn environment_meeting_its_host_count(router: &Emulator, case: &str, name: &str) -> String {
    let mut request = router.create_request(Some(name));
    request["hosts"] = (0..evs::MINIMUM_HOST_COUNT)
        .map(|host| {
            json!({
                "hostName": format!("{name}-{host}"),
                "keyName": "evs-key",
                "instanceType": "i4i.metal",
            })
        })
        .collect();
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create {name}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let id = body["environment"]["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: create {name}: no environmentId in {body}"))
        .to_string();
    settle(router);
    id
}

/// The `environment` object `GetEnvironment` reports.
async fn get_environment(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get {environment_id}: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"].clone()
}

/// The connectors `ListEnvironmentConnectors` reports for one environment.
async fn listed_connectors(router: &Emulator, case: &str, environment_id: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the list expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no connectors list in {body}"))
        .clone()
}

/// The environment's `CONNECTOR_HEALTH` check, or `Value::Null` when it reports none.
fn connector_health_check(environment: &Value) -> Value {
    environment["checks"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|check| check["type"] == json!(CONNECTOR_HEALTH))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert the environment reports exactly one `CONNECTOR_HEALTH` check, carrying
/// `expected`, the id of that check of that environment, and an `impairedSince` exactly
/// when it failed. Returns that check.
fn assert_aggregate(case: &str, environment_id: &str, environment: &Value, expected: &str) -> Value {
    let checks = environment["checks"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a settled environment reports checks, got {environment}")
        })
        .clone();
    let reported: Vec<&Value> = checks
        .iter()
        .filter(|check| check["type"] == json!(CONNECTOR_HEALTH))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the environment reports ONE aggregate connector-health check, got \
         {reported:?} in {checks:?}"
    );
    let check = connector_health_check(environment);
    assert_eq!(
        check["result"],
        json!(expected),
        "case {case:?}: expected the aggregate {expected}, got {check}"
    );
    assert_eq!(
        check["id"],
        json!(evs::check_id(environment_id, CONNECTOR_HEALTH)),
        "case {case:?}: expected the id of this check of this environment, got {check}"
    );
    assert_eq!(
        !check["impairedSince"].is_null(),
        expected == FAILED,
        "case {case:?}: a {expected} aggregate carries impairedSince {}",
        check["impairedSince"]
    );
    check
}

/// One row of the table: the connectors the environment holds, each as the
/// `ConnectorType` it connects to and the `ConnectorState` its record is driven into.
struct Row {
    case: &'static str,
    connectors: Vec<(&'static str, &'static str)>,
    expected: &'static str,
}

/// Create one connector and drive its record into `state` the way a client reaches it,
/// writing the two states no operation of this service produces straight into the store
/// (as `connector_health_checks` does, which owns that drive).
async fn connector_in_state(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_type: &str,
    state: &str,
) {
    let fqdn = format!(
        "{}.evs.example.com",
        connector_type.to_ascii_lowercase().replace('_', "-")
    );
    let created = create_connector(
        router,
        case,
        environment_id,
        connector_type,
        &fqdn,
        SECRET_ARN,
    )
    .await;
    let connector_id = created["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {created}"))
        .to_string();
    match state {
        // The connection is still being established: the row creates it last, so nothing
        // settles it afterwards.
        "CREATING" => {}
        "ACTIVE" => settle(router),
        "CREATE_FAILED" | "UPDATE_FAILED" => {
            settle(router);
            force_connector_state(router, environment_id, &connector_id, state);
        }
        "DELETING" | "DELETED" => {
            settle(router);
            let (status, body) = call(
                router,
                "DeleteEnvironmentConnector",
                &json!({ "environmentId": environment_id, "connectorId": connector_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the delete expected HTTP 200, got {status} ({}) {body}",
                error_type(&body)
            );
            if state == "DELETED" {
                settle(router);
            }
        }
        other => panic!("case {case:?}: this suite drives no record into {other:?}"),
    }
}

#[tokio::test]
async fn connector_health_is_the_aggregate_over_the_environments_connectors() {
    let router = emulator();
    let rows = vec![
        Row {
            case: "an environment holding no connector at all",
            connectors: vec![],
            expected: UNKNOWN,
        },
        Row {
            case: "one connector whose connection is still being established",
            connectors: vec![("VCENTER", "CREATING")],
            expected: UNKNOWN,
        },
        Row {
            case: "one connector whose connection stands",
            connectors: vec![("VCENTER", "ACTIVE")],
            expected: PASSED,
        },
        Row {
            case: "two connectors, both connected",
            connectors: vec![("VCENTER", "ACTIVE"), ("SDDC_MANAGER", "ACTIVE")],
            expected: PASSED,
        },
        Row {
            case: "one connector of every modelled type, all connected",
            connectors: vec![
                ("VCENTER", "ACTIVE"),
                ("SDDC_MANAGER", "ACTIVE"),
                ("OPERATIONS_MANAGER", "ACTIVE"),
            ],
            expected: PASSED,
        },
        Row {
            case: "a connected connector beside one whose connection failed",
            connectors: vec![("VCENTER", "ACTIVE"), ("SDDC_MANAGER", "CREATE_FAILED")],
            expected: FAILED,
        },
        Row {
            case: "the only connector failed to connect",
            connectors: vec![("OPERATIONS_MANAGER", "CREATE_FAILED")],
            expected: FAILED,
        },
        Row {
            case: "two connectors, each concluded without a connection",
            connectors: vec![
                ("VCENTER", "CREATE_FAILED"),
                ("OPERATIONS_MANAGER", "UPDATE_FAILED"),
            ],
            expected: FAILED,
        },
        Row {
            case: "a connected connector beside one still being established",
            connectors: vec![("SDDC_MANAGER", "ACTIVE"), ("VCENTER", "CREATING")],
            expected: PASSED,
        },
        Row {
            case: "the only connector is being torn down",
            connectors: vec![("VCENTER", "DELETING")],
            expected: UNKNOWN,
        },
        Row {
            case: "the only connector has been deleted",
            connectors: vec![("VCENTER", "DELETED")],
            expected: UNKNOWN,
        },
        Row {
            case: "a connected connector beside a deleted one",
            connectors: vec![("SDDC_MANAGER", "ACTIVE"), ("VCENTER", "DELETED")],
            expected: PASSED,
        },
    ];
    // Every modelled type appears in the table, so the catalog and the rows cannot drift
    // apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter().any(|row| row
                .connectors
                .iter()
                .any(|(row_type, _)| row_type == connector_type)),
            "the table must cover the modelled connector type {connector_type:?}"
        );
    }

    for (index, row) in rows.into_iter().enumerate() {
        let Row {
            case,
            connectors,
            expected,
        } = row;
        let environment_id =
            environment_meeting_its_host_count(&router, case, &format!("aggregate-{index}")).await;
        // A connector left mid-flight is created LAST, so settling another row member
        // cannot settle it too.
        let (in_flight, settled): (Vec<_>, Vec<_>) = connectors
            .into_iter()
            .partition(|(_, state)| *state == "CREATING");
        for (connector_type, state) in settled.into_iter().chain(in_flight) {
            connector_in_state(&router, case, &environment_id, connector_type, state).await;
        }

        let environment = get_environment(&router, case, &environment_id).await;
        assert_aggregate(case, &environment_id, &environment, expected);

        // The environment's other checks cannot fail here, so the roll-up reports exactly
        // what the aggregate contributes.
        let status = environment["environmentStatus"].as_str().unwrap_or_default();
        let expected_status = if expected == FAILED { FAILED } else { PASSED };
        assert_eq!(
            status, expected_status,
            "case {case:?}: environmentStatus must roll the checks up to {expected_status}, got \
             {status} from {}",
            environment["checks"]
        );

        // The two surfaces agree: the aggregate is the roll-up of the per-connector
        // statuses the list reports, and no connector reports the aggregate's own type.
        let listed = listed_connectors(&router, case, &environment_id).await;
        let statuses: Vec<&str> = listed
            .iter()
            .map(|connector| connector["status"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            rolled_up(&statuses),
            expected,
            "case {case:?}: the aggregate must roll up the connectors' own statuses \
             {statuses:?}, expected {expected}"
        );
        for connector in &listed {
            let empty = Vec::new();
            let types: Vec<&str> = connector["checks"]
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|check| check["type"].as_str().unwrap_or("<none>"))
                .collect();
            assert!(
                !types.contains(&CONNECTOR_HEALTH),
                "case {case:?}: {CONNECTOR_HEALTH} aggregates the whole environment, so no \
                 single connector reports it, got {types:?} on {connector}"
            );
        }

        // A read changes nothing a later caller sees.
        let again = get_environment(&router, case, &environment_id).await;
        assert_eq!(
            again["checks"], environment["checks"],
            "case {case:?}: a second read reported different checks: {} then {}",
            environment["checks"], again["checks"]
        );
    }
}

#[tokio::test]
async fn the_aggregates_impairment_keeps_its_onset_until_every_connector_is_healthy_again() {
    let router = emulator();
    let case = "an environment whose connector stopped connecting";
    let environment_id = environment_meeting_its_host_count(&router, case, "impaired").await;

    // One connected connector: the aggregate passes and nothing has been impaired.
    connector_in_state(&router, case, &environment_id, "VCENTER", "ACTIVE").await;
    let environment = get_environment(&router, case, &environment_id).await;
    assert_aggregate(case, &environment_id, &environment, PASSED);
    let first = listed_connectors(&router, case, &environment_id).await;
    let connector_id = first[0]["connectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {:?}", first[0]))
        .to_string();

    // That connector's connection concludes without a connection: the aggregate is
    // impaired, from no earlier than the environment existed.
    force_connector_state(&router, &environment_id, &connector_id, "CREATE_FAILED");
    let environment = get_environment(&router, case, &environment_id).await;
    let check = assert_aggregate(case, &environment_id, &environment, FAILED);
    let onset = check["impairedSince"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: an impaired aggregate reports impairedSince, got {check}"));
    let created_at = environment["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no createdAt in {environment}"));
    assert!(
        onset >= created_at,
        "case {case:?}: the impairment began at {onset}, before the environment existed at \
         {created_at}"
    );

    // Adding a healthy connector neither clears the impairment nor re-dates it: the
    // aggregate is over ALL of them.
    connector_in_state(&router, case, &environment_id, "SDDC_MANAGER", "ACTIVE").await;
    let environment = get_environment(&router, case, &environment_id).await;
    let check = assert_aggregate(case, &environment_id, &environment, FAILED);
    assert_eq!(
        check["impairedSince"].as_f64(),
        Some(onset),
        "case {case:?}: the impairment must keep the onset it began with, got {check}"
    );

    // Once the impaired connector connects again the aggregate passes, onset and all.
    force_connector_state(&router, &environment_id, &connector_id, "ACTIVE");
    let environment = get_environment(&router, case, &environment_id).await;
    let check = assert_aggregate(case, &environment_id, &environment, PASSED);
    assert!(
        check["impairedSince"].is_null(),
        "case {case:?}: an aggregate that passes carries no impairedSince, got {check}"
    );
}
