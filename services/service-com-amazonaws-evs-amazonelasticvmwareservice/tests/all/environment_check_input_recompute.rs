// SPDX-License-Identifier: CC0-1.0

//! The environment health checks a call that CHANGED one of their inputs leaves behind,
//! end to end over awsJson1_0.
//!
//! THE RULE under test: `Environment$checks` is derived from the environment, the hosts
//! and the connectors it holds and the account's own settings, and `environmentStatus`
//! "reports impaired functionality that stems from issues internal to the environment"
//! as the roll-up over those checks (`smithy_query(query="shape",
//! shape="Check,CheckType,CheckResult")` and the `Environment`/`EnvironmentSummary`
//! members). So an operation that changes one of those facts recomputes the checks from
//! the state it leaves behind: the environment it reports carries the status that holds
//! AFTER its change, and the checks left in the store are the ones the next read reports.
//!
//! Which fact each check reads, and so what counts as changing one:
//!
//! * `CONNECTOR_HEALTH` — "Aggregate health across all connectors in the environment"
//!   (`Check$type`): the roll-up of what each connector has decided about its own
//!   connectivity, so a connector created, reconfigured or torn down changes it.
//! * `HOST_COUNT` and `KEY_COVERAGE` — the hosts the environment is deployed with, so a
//!   host added or removed changes them.
//! * `KEY_COVERAGE` also reads "The total number of VCF license cores ported to Amazon
//!   EVS for the account in that Region" (`AccountSetting$name`), so writing that setting
//!   changes a check input of every environment of the Region.
//!
//! The rows of the first table differ along what the rule generalises over: which
//! operation performs the change, which check input it changes, how many connectors the
//! environment holds and which `ConnectorState` each is in, and whether the roll-up
//! changes, stays impaired or stays healthy across the call. The second table runs the
//! account-settings input over both sides of the comparison and its exact boundary.
//!
//! The harness (`emulator`, `call`, `settle`) is the one `environment_lifecycle` owns and
//! the connector helpers are `connector_health_checks`', so every test drives the service
//! the way a client does: `POST /` with `X-Amz-Target:
//! AmazonElasticVMwareService.<Operation>`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state as evs;

use crate::connector_health_checks::{create_connector, force_connector_state};
use crate::environment_lifecycle::{call, emulator, error_type, settle, Emulator};

/// The `CheckResult` values, written out from the model's own enumeration
/// (`smithy_query(query="shape", shape="CheckResult")`), so this suite states the roll-up
/// instead of reading it back out of the code under test.
const PASSED: &str = "PASSED";
/// See [`PASSED`].
const FAILED: &str = "FAILED";
/// See [`PASSED`].
const UNKNOWN: &str = "UNKNOWN";

/// The check types this suite names, from the service's own catalog of them.
const CONNECTOR_HEALTH: &str = evs::CHECK_TYPE_CONNECTOR_HEALTH;
/// See [`CONNECTOR_HEALTH`].
const KEY_COVERAGE: &str = evs::CHECK_TYPE_KEY_COVERAGE;

/// A secret given as a full Secrets Manager ARN, one form `secretIdentifier` accepts.
const SECRET_ARN: &str = "arn:aws:secretsmanager:us-west-2:000000000000:secret:evs/vcf-AbCd12";

/// The bare metal instance type every host of this suite runs on, one value of the
/// modelled `InstanceType` enum.
pub(crate) const INSTANCE_TYPE: &str = "i4i.metal";

/// The appliance FQDN an update reconfigures a connector onto.
const RECONFIGURED_FQDN: &str = "reconfigured.evs.example.com";

/// An appliance FQDN for one connector type, so each connector of a row carries its own.
fn fqdn_for(connector_type: &str) -> String {
    format!(
        "{}.evs.example.com",
        connector_type.to_ascii_lowercase().replace('_', "-")
    )
}

/// One VCF license key of the modelled shape
/// (`^[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}-[a-zA-Z0-9]{5}$`).
pub(crate) fn license_info() -> Value {
    json!([{ "solutionKey": "AAAAA-AAAAA-AAAAA-AAAAA-AAAAA", "vsanKey": "BBBBB-BBBBB-BBBBB-BBBBB-BBBBB" }])
}

/// The physical processor cores one host running on `instance_type` has, as Amazon EC2's
/// own instance-type record reports them (`VCpuInfo$DefaultCores`) — the same fact the
/// check reads, so this suite pins the RULE and not a core count copied by hand.
pub(crate) fn cores(instance_type: &str) -> u64 {
    let record = ec2::state::instance_type_record(instance_type).unwrap_or_else(|| {
        panic!("Amazon EC2 describes the Amazon EVS instance type {instance_type}")
    });
    u64::try_from(record.default_cores).expect("a core count is not negative")
}

/// An environment deployed with exactly the minimum host count, settled, with `extra`
/// merged into the request every rule accepts. Its hosts are named after it, because
/// "DNS hostnames for hosts must be unique across Amazon EVS environments"
/// (`HostInfoForCreate$hostName` doc).
async fn environment_at_its_minimum_host_count(
    router: &Emulator,
    case: &str,
    name: &str,
    extra: Value,
) -> String {
    let mut request = router.create_request(Some(name));
    request["hosts"] = (0..evs::MINIMUM_HOST_COUNT)
        .map(|ordinal| {
            json!({
                "hostName": format!("{name}-{ordinal}"),
                "keyName": "evs-key",
                "instanceType": INSTANCE_TYPE,
            })
        })
        .collect();
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        request[member] = value.clone();
    }
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
pub(crate) async fn get_environment(router: &Emulator, case: &str, environment_id: &str) -> Value {
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

/// The summary `ListEnvironments` reports for one environment.
pub(crate) async fn listed_summary(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(router, "ListEnvironments", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEnvironments expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environmentSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no environmentSummaries in {body}"))
        .iter()
        .find(|summary| summary["environmentId"] == json!(environment_id))
        .unwrap_or_else(|| panic!("case {case:?}: no summary for {environment_id} in {body}"))
        .clone()
}

/// The (check type, result) pairs the STORE holds for one environment, read straight out
/// of it — so what a call left behind is observed without a read of the service
/// recomputing it first.
pub(crate) fn stored_check_results(
    router: &Emulator,
    environment_id: &str,
) -> Vec<(String, String)> {
    let store = evs::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = store.lock();
    let environment = store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"));
    environment
        .checks
        .iter()
        .map(|check| {
            (
                check.r#type.clone().unwrap_or_default(),
                check.result.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The same pairs, as a reported `Environment` carries them.
pub(crate) fn reported_check_results(environment: &Value) -> Vec<(String, String)> {
    environment["checks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|check| {
            (
                check["type"].as_str().unwrap_or_default().to_string(),
                check["result"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One check's result out of those pairs.
pub(crate) fn result_of(checks: &[(String, String)], check_type: &str) -> String {
    checks
        .iter()
        .find(|(reported, _)| reported == check_type)
        .map(|(_, result)| result.clone())
        .unwrap_or_else(|| "<none>".to_string())
}

/// The `environmentStatus` of a reported environment or summary.
fn status_of(reported: &Value) -> String {
    reported["environmentStatus"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

/// The change a row performs — one operation, each changing a fact the environment's
/// checks read.
#[derive(Debug)]
enum Mutation {
    /// `DeleteEnvironmentConnector` on the connector of this type: it goes `DELETING`, so
    /// it decides nothing about connectivity any more.
    DeleteConnector(&'static str),
    /// `UpdateEnvironmentConnector` on the connector of this type: it reconnects, so it
    /// decides nothing until it is active again.
    UpdateConnector(&'static str),
    /// `CreateEnvironmentConnector` of this type, which adds a connector to the aggregate.
    CreateConnector(&'static str),
    /// `DeleteEnvironmentHost` on the environment's first host.
    DeleteHost,
    /// `CreateEnvironmentHost`, which adds one host to the deployed population.
    CreateHost,
}

impl Mutation {
    /// The member of the response that carries the environment, for the operations whose
    /// output models one (`smithy_query(query="operation",
    /// operation="DeleteEnvironmentConnector,DeleteEnvironmentHost,CreateEnvironmentHost")`).
    fn reports_environment_summary(&self) -> bool {
        matches!(
            self,
            Mutation::DeleteConnector(_) | Mutation::DeleteHost | Mutation::CreateHost
        )
    }
}

/// One row of the first table.
struct Row {
    case: &'static str,
    /// The connectors the environment holds before the change, each as its
    /// `ConnectorType` and the `ConnectorState` its record is driven into.
    connectors: Vec<(&'static str, &'static str)>,
    mutation: Mutation,
    /// The roll-up the environment reports BEFORE the change, asserted so each row states
    /// the health its change starts from.
    status_before: &'static str,
    /// `CONNECTOR_HEALTH` after the change.
    connector_health_after: &'static str,
    /// The roll-up after the change.
    status_after: &'static str,
}

/// Create the row's connectors and return the id of each, by type. A record in a state no
/// operation of this service produces is driven there the way `connector_health_checks`
/// does, which owns that drive.
async fn connectors_in_state(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connectors: &[(&'static str, &'static str)],
) -> BTreeMap<&'static str, String> {
    let mut ids = BTreeMap::new();
    for (connector_type, state) in connectors {
        let created = create_connector(
            router,
            case,
            environment_id,
            connector_type,
            &fqdn_for(connector_type),
            SECRET_ARN,
        )
        .await;
        let connector_id = created["connectorId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {created}"))
            .to_string();
        settle(router);
        match *state {
            "ACTIVE" => {}
            "CREATE_FAILED" | "UPDATE_FAILED" => {
                force_connector_state(router, environment_id, &connector_id, state);
            }
            other => panic!("case {case:?}: this suite drives no record into {other:?}"),
        }
        ids.insert(*connector_type, connector_id);
    }
    ids
}

/// Perform the row's change and return the response body.
async fn apply(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    host_prefix: &str,
    mutation: &Mutation,
    connector_ids: &BTreeMap<&'static str, String>,
) -> Value {
    let connector_id = |connector_type: &'static str| -> String {
        connector_ids
            .get(connector_type)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the row holds a {connector_type} connector")
            })
            .clone()
    };
    let (operation, request) = match mutation {
        Mutation::DeleteConnector(connector_type) => (
            "DeleteEnvironmentConnector",
            json!({
                "environmentId": environment_id,
                "connectorId": connector_id(connector_type),
            }),
        ),
        Mutation::UpdateConnector(connector_type) => (
            "UpdateEnvironmentConnector",
            json!({
                "environmentId": environment_id,
                "connectorId": connector_id(connector_type),
                "applianceFqdn": RECONFIGURED_FQDN,
            }),
        ),
        Mutation::CreateConnector(connector_type) => (
            "CreateEnvironmentConnector",
            json!({
                "environmentId": environment_id,
                "type": connector_type,
                "applianceFqdn": fqdn_for(connector_type),
                "secretIdentifier": SECRET_ARN,
            }),
        ),
        Mutation::DeleteHost => (
            "DeleteEnvironmentHost",
            json!({
                "environmentId": environment_id,
                "hostName": format!("{host_prefix}-0"),
            }),
        ),
        Mutation::CreateHost => (
            "CreateEnvironmentHost",
            json!({
                "environmentId": environment_id,
                "host": {
                    "hostName": format!("{host_prefix}-added"),
                    "keyName": "evs-key",
                    "instanceType": INSTANCE_TYPE,
                },
            }),
        ),
    };
    let (status, body) = call(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body
}

#[tokio::test]
async fn a_call_that_changes_a_check_input_reports_and_stores_the_health_that_holds_after_it() {
    let router = emulator();
    // Every environment here is deployed with the minimum host count and carries no
    // license key, so `HOST_COUNT` passes and the two license checks are undecidable: the
    // roll-up reports exactly what the connector aggregate contributes.
    let rows = vec![
        Row {
            case: "the only connector, which had failed to connect, is torn down",
            connectors: vec![("VCENTER", "CREATE_FAILED")],
            mutation: Mutation::DeleteConnector("VCENTER"),
            status_before: FAILED,
            connector_health_after: UNKNOWN,
            status_after: PASSED,
        },
        Row {
            case: "the only connector, whose reconfiguration had failed, is torn down",
            connectors: vec![("OPERATIONS_MANAGER", "UPDATE_FAILED")],
            mutation: Mutation::DeleteConnector("OPERATIONS_MANAGER"),
            status_before: FAILED,
            connector_health_after: UNKNOWN,
            status_after: PASSED,
        },
        Row {
            case: "the failed connector beside a connected one is torn down",
            connectors: vec![("VCENTER", "CREATE_FAILED"), ("SDDC_MANAGER", "ACTIVE")],
            mutation: Mutation::DeleteConnector("VCENTER"),
            status_before: FAILED,
            connector_health_after: PASSED,
            status_after: PASSED,
        },
        Row {
            case: "the connected connector is torn down and the failed one stays",
            connectors: vec![("VCENTER", "ACTIVE"), ("SDDC_MANAGER", "CREATE_FAILED")],
            mutation: Mutation::DeleteConnector("VCENTER"),
            status_before: FAILED,
            connector_health_after: FAILED,
            status_after: FAILED,
        },
        Row {
            case: "the only connector, which was connected, is torn down",
            connectors: vec![("VCENTER", "ACTIVE")],
            mutation: Mutation::DeleteConnector("VCENTER"),
            status_before: PASSED,
            connector_health_after: UNKNOWN,
            status_after: PASSED,
        },
        Row {
            case: "the connected connector is reconfigured onto another appliance",
            connectors: vec![("VCENTER", "ACTIVE")],
            mutation: Mutation::UpdateConnector("VCENTER"),
            status_before: PASSED,
            connector_health_after: UNKNOWN,
            status_after: PASSED,
        },
        Row {
            case: "a second connector is created beside one that failed to connect",
            connectors: vec![("VCENTER", "CREATE_FAILED")],
            mutation: Mutation::CreateConnector("SDDC_MANAGER"),
            status_before: FAILED,
            connector_health_after: FAILED,
            status_after: FAILED,
        },
        Row {
            case: "the first connector of the environment is created",
            connectors: vec![],
            mutation: Mutation::CreateConnector("VCENTER"),
            status_before: PASSED,
            connector_health_after: UNKNOWN,
            status_after: PASSED,
        },
        Row {
            case: "a host is removed while a connector is still connected",
            connectors: vec![("SDDC_MANAGER", "ACTIVE")],
            mutation: Mutation::DeleteHost,
            status_before: PASSED,
            connector_health_after: PASSED,
            status_after: PASSED,
        },
        Row {
            case: "a host is added while a connector has failed to connect",
            connectors: vec![("VCENTER", "CREATE_FAILED")],
            mutation: Mutation::CreateHost,
            status_before: FAILED,
            connector_health_after: FAILED,
            status_after: FAILED,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let Row {
            case,
            connectors,
            mutation,
            status_before,
            connector_health_after,
            status_after,
        } = row;
        let name = format!("recompute-{index}");
        let environment_id =
            environment_at_its_minimum_host_count(&router, case, &name, json!({})).await;
        let connector_ids = connectors_in_state(&router, case, &environment_id, connectors).await;

        // The health the change starts from, as a client reads it.
        let before = get_environment(&router, case, &environment_id).await;
        assert_eq!(
            status_of(&before),
            *status_before,
            "case {case:?}: before the change: expected {status_before}, got {} from {}",
            status_of(&before),
            before["checks"]
        );

        let body = apply(
            &router,
            case,
            &environment_id,
            &name,
            mutation,
            &connector_ids,
        )
        .await;
        // What the call LEFT in the store, captured before any read recomputes it.
        let left_behind = stored_check_results(&router, &environment_id);
        assert_eq!(
            result_of(&left_behind, CONNECTOR_HEALTH),
            *connector_health_after,
            "case {case:?}: after {mutation:?} the stored {CONNECTOR_HEALTH} must be \
             {connector_health_after}, got {left_behind:?}"
        );

        // The checks a read taken straight afterwards reports are the ones the call left
        // behind: the change is accounted for once, by the call that made it.
        let after = get_environment(&router, case, &environment_id).await;
        let reported = reported_check_results(&after);
        assert_eq!(
            left_behind, reported,
            "case {case:?}: after {mutation:?} the stored checks {left_behind:?} must be the \
             checks the next read reports, got {reported:?}"
        );
        assert_eq!(
            status_of(&after),
            *status_after,
            "case {case:?}: after {mutation:?}: expected {status_after}, got {} from {}",
            status_of(&after),
            after["checks"]
        );

        // The environment the call itself answered with is that same environment, status
        // and all — the response and a read of one record cannot disagree.
        if mutation.reports_environment_summary() {
            let summary = body["environmentSummary"].clone();
            assert_eq!(
                status_of(&summary),
                *status_after,
                "case {case:?}: {mutation:?} reported the status {} that held before its \
                 change, expected {status_after}: {body}",
                status_of(&summary)
            );
            assert_eq!(
                summary,
                listed_summary(&router, case, &environment_id).await,
                "case {case:?}: the summary must be the environment's own, got {body}"
            );
        } else {
            assert!(
                body["environmentSummary"].is_null(),
                "case {case:?}: {mutation:?} models no environmentSummary, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn writing_the_accounts_ported_core_count_recomputes_what_it_is_a_check_input_of() {
    let router = emulator();
    let case = "an environment whose license has to cover its deployed hosts";
    let environment_id = environment_at_its_minimum_host_count(
        &router,
        case,
        "ported-cores",
        json!({ "licenseInfo": license_info() }),
    )
    .await;
    // The cores the environment's hosts run on: what `KEY_COVERAGE` compares the account's
    // ported cores against.
    let required = cores(INSTANCE_TYPE) * u64::try_from(evs::MINIMUM_HOST_COUNT).unwrap();

    // Each row: the ported core count the account states, and the check result it decides
    // — both sides of the comparison and its exact boundary ("sufficient vCPU cores for
    // all deployed hosts", so a count equal to what the hosts need is sufficient).
    let rows = [
        (required - 1, FAILED),
        (required, PASSED),
        (required + 1, PASSED),
        (required * 8, PASSED),
    ];
    for (ported, expected) in rows {
        let case = format!("{case}, {ported} core(s) ported against {required} required");
        let (status, body) = call(
            &router,
            "PutAccountSettings",
            &json!({
                "settings": [{ "name": evs::VCF_PORTED_CORE_COUNT, "value": ported.to_string() }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );

        // What the put LEFT in the store, captured before any read recomputes it.
        let left_behind = stored_check_results(&router, &environment_id);
        assert_eq!(
            result_of(&left_behind, KEY_COVERAGE),
            expected,
            "case {case:?}: the stored {KEY_COVERAGE} must be {expected}, got {left_behind:?}"
        );

        let after = get_environment(&router, &case, &environment_id).await;
        let reported = reported_check_results(&after);
        assert_eq!(
            left_behind, reported,
            "case {case:?}: the stored checks {left_behind:?} must be the checks the next read \
             reports, got {reported:?}"
        );
        let expected_status = if expected == FAILED { FAILED } else { PASSED };
        assert_eq!(
            status_of(&after),
            expected_status,
            "case {case:?}: expected the roll-up {expected_status}, got {} from {}",
            status_of(&after),
            after["checks"]
        );
    }
}
