// SPDX-License-Identifier: CC0-1.0

//! What a change to a record an environment HOLDS does to the environment's own
//! last-modified stamp, over awsJson1_0.
//!
//! The rule under test: `Environment$modifiedAt` and `EnvironmentSummary$modifiedAt` are
//! "The date and time that the environment was modified"
//! (`smithy_query(query="doc",shape="Environment",member="modifiedAt")`), and the
//! records an environment holds — its ESX hosts, its connectors, its entitlements and
//! the Elastic IP associations of its VLANs — are part of the environment, so EVERY
//! accepted mutation of one of them modifies the environment and moves its `modifiedAt`,
//! in either direction: adding a child record and removing one move it alike. A call that
//! changes nothing — an idempotent repeat, or a request that is refused — leaves the
//! stamp exactly where it was.
//!
//! What the rule generalises over, and so what the table below spans: the KIND of child
//! record (host, connector, entitlement, VLAN Elastic IP association), the DIRECTION of
//! the change (added, reconfigured, removed), and whether the call really changed
//! anything (a first call, an idempotent repeat, a refused request). The moving rows and
//! the still rows sit in one table, so what separates them is whether the call changed a
//! record and not the shape of the request.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads the environment back
//! through `GetEnvironment`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, Emulator, PAST_THE_TRANSITION,
};
use crate::hcx_eip_associations::{create_public_hcx, register_elastic_ip};

/// How far apart the calls of the wall-clock test sit, so a stamp written by the later
/// one is distinguishable from the earlier one's.
const BETWEEN_CALLS: Duration = Duration::from_millis(25);

/// How far into the past a baseline stamp is moved before the call under test, so a
/// stamp written from the current clock is distinguishable from the baseline whatever
/// the clock's resolution — a minute, far longer than a test takes.
const BACKDATE_MS: i64 = 60_000;

/// The connector type an entitlement is created through
/// (`smithy_query(query="shape",shape="ConnectorType")` declares it).
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape",shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The credential secret every connector here is created with.
const SECRET_IDENTIFIER: &str = "evs/vcf-appliance-credentials";

/// The instance type every host here runs on, and the key pair it is launched with.
const INSTANCE_TYPE: &str = "i4i.metal";
const KEY_NAME: &str = "evs-key";

/// One change to a record an environment holds, as the request that drives it.
#[derive(Debug, Clone)]
enum Mutation {
    /// `CreateEnvironmentHost`: add the named ESX host.
    AddHost(&'static str),
    /// `DeleteEnvironmentHost`: remove the named ESX host.
    RemoveHost(&'static str),
    /// `CreateEnvironmentConnector`: add a connector reaching the named appliance.
    AddConnector(&'static str),
    /// `UpdateEnvironmentConnector`: point the environment's connector at another
    /// appliance FQDN.
    ReconfigureConnector(&'static str),
    /// `DeleteEnvironmentConnector`: remove the environment's connector.
    RemoveConnector,
    /// `CreateEntitlement`: entitle the named virtual machines.
    AddEntitlements(&'static [&'static str]),
    /// `DeleteEntitlement`: stop tracking the named virtual machines.
    RemoveEntitlements(&'static [&'static str]),
    /// `AssociateEipToVlan`: associate the named Elastic IP allocation with the public
    /// HCX VLAN.
    AssociateEip(&'static str),
    /// `DisassociateEipFromVlan`: remove the association the last associate recorded.
    DisassociateEip,
}

/// The ids a row's later mutations address, as the earlier ones minted them.
struct Subject {
    environment: String,
    connector: Option<String>,
    association: Option<String>,
}

/// Drive one mutation against `subject` and answer the call's status and body, recording
/// the ids it minted so a later mutation of the same row can address them.
async fn apply(
    router: &Emulator,
    case: &str,
    subject: &mut Subject,
    mutation: &Mutation,
) -> (u16, Value) {
    let environment = subject.environment.clone();
    match mutation {
        Mutation::AddHost(host_name) => {
            call(
                router,
                "CreateEnvironmentHost",
                &json!({
                    "environmentId": environment,
                    "host": {
                        "hostName": host_name,
                        "keyName": KEY_NAME,
                        "instanceType": INSTANCE_TYPE,
                    },
                }),
            )
            .await
        }
        Mutation::RemoveHost(host_name) => {
            call(
                router,
                "DeleteEnvironmentHost",
                &json!({ "environmentId": environment, "hostName": host_name }),
            )
            .await
        }
        Mutation::AddConnector(appliance_fqdn) => {
            let (status, body) = call(
                router,
                "CreateEnvironmentConnector",
                &json!({
                    "environmentId": environment,
                    "type": VCENTER,
                    "applianceFqdn": appliance_fqdn,
                    "secretIdentifier": SECRET_IDENTIFIER,
                }),
            )
            .await;
            if let Some(connector_id) = body["connector"]["connectorId"].as_str() {
                subject.connector = Some(connector_id.to_string());
            }
            (status, body)
        }
        Mutation::ReconfigureConnector(appliance_fqdn) => {
            let connector = connector_of(case, subject);
            call(
                router,
                "UpdateEnvironmentConnector",
                &json!({
                    "environmentId": environment,
                    "connectorId": connector,
                    "applianceFqdn": appliance_fqdn,
                }),
            )
            .await
        }
        Mutation::RemoveConnector => {
            let connector = connector_of(case, subject);
            call(
                router,
                "DeleteEnvironmentConnector",
                &json!({ "environmentId": environment, "connectorId": connector }),
            )
            .await
        }
        Mutation::AddEntitlements(vm_ids) => {
            let connector = connector_of(case, subject);
            call(
                router,
                "CreateEntitlement",
                &json!({
                    "environmentId": environment,
                    "connectorId": connector,
                    "entitlementType": WINDOWS_SERVER,
                    "vmIds": vm_ids,
                }),
            )
            .await
        }
        Mutation::RemoveEntitlements(vm_ids) => {
            let connector = connector_of(case, subject);
            call(
                router,
                "DeleteEntitlement",
                &json!({
                    "environmentId": environment,
                    "connectorId": connector,
                    "entitlementType": WINDOWS_SERVER,
                    "vmIds": vm_ids,
                }),
            )
            .await
        }
        Mutation::AssociateEip(allocation_id) => {
            // The allocation has to be one the account's own Amazon EC2 store holds,
            // because an id it holds no allocation for is refused.
            register_elastic_ip(&router.engine, allocation_id, Some("198.51.100.7"));
            let (status, body) = call(
                router,
                "AssociateEipToVlan",
                &json!({
                    "environmentId": environment,
                    "vlanName": "hcx",
                    "allocationId": allocation_id,
                }),
            )
            .await;
            if let Some(association_id) = association_id_of(&body, allocation_id) {
                subject.association = Some(association_id);
            }
            (status, body)
        }
        Mutation::DisassociateEip => {
            let association = subject
                .association
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: no association has been recorded yet"));
            call(
                router,
                "DisassociateEipFromVlan",
                &json!({
                    "environmentId": environment,
                    "vlanName": "hcx",
                    "associationId": association,
                }),
            )
            .await
        }
    }
}

/// The connector a row's connector-addressed mutation names, as its `AddConnector`
/// minted it.
fn connector_of(case: &str, subject: &Subject) -> String {
    subject
        .connector
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: no connector has been created yet"))
}

/// The `associationId` a reported VLAN carries for `allocation_id`, if any.
fn association_id_of(body: &Value, allocation_id: &str) -> Option<String> {
    body["vlan"]["eipAssociations"]
        .as_array()?
        .iter()
        .find(|association| association["allocationId"] == json!(allocation_id))
        .and_then(|association| association["associationId"].as_str())
        .map(str::to_string)
}

/// One row: the child records the environment starts with, the one call under test, and
/// whether that call is a modification of the environment.
struct Row {
    case: &'static str,
    setup: Vec<Mutation>,
    under_test: Mutation,
    /// The status the call under test answers with: a refused request is a row too.
    expect_status: u16,
    expect_stamp: bool,
}

fn rows() -> Vec<Row> {
    vec![
        // A child record is ADDED.
        Row {
            case: "a host added",
            setup: Vec::new(),
            under_test: Mutation::AddHost("esx-added"),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "a second host added beside one",
            setup: vec![Mutation::AddHost("esx-first")],
            under_test: Mutation::AddHost("esx-second"),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "a connector added",
            setup: Vec::new(),
            under_test: Mutation::AddConnector("vcenter.evs.example.com"),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "entitlements added",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::AddEntitlements(&["vm-100", "vm-101"]),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "an Elastic IP associated with the public HCX VLAN",
            setup: Vec::new(),
            under_test: Mutation::AssociateEip("eipalloc-00000000000000001"),
            expect_status: 200,
            expect_stamp: true,
        },
        // A child record is RECONFIGURED.
        Row {
            case: "a connector pointed at another appliance",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::ReconfigureConnector("vcenter-moved.evs.example.com"),
            expect_status: 200,
            expect_stamp: true,
        },
        // A child record is REMOVED — the inverse of each addition above.
        Row {
            case: "the only host removed",
            setup: vec![Mutation::AddHost("esx-only")],
            under_test: Mutation::RemoveHost("esx-only"),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "one of two hosts removed",
            setup: vec![
                Mutation::AddHost("esx-kept"),
                Mutation::AddHost("esx-removed"),
            ],
            under_test: Mutation::RemoveHost("esx-removed"),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "a connector removed",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::RemoveConnector,
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "entitlements removed",
            setup: vec![
                Mutation::AddConnector("vcenter.evs.example.com"),
                Mutation::AddEntitlements(&["vm-100", "vm-101"]),
            ],
            under_test: Mutation::RemoveEntitlements(&["vm-100"]),
            expect_status: 200,
            expect_stamp: true,
        },
        Row {
            case: "an Elastic IP disassociated from the public HCX VLAN",
            setup: vec![Mutation::AssociateEip("eipalloc-00000000000000002")],
            under_test: Mutation::DisassociateEip,
            expect_status: 200,
            expect_stamp: true,
        },
        // Nothing changes: an idempotent repeat reports the records as they stand.
        Row {
            case: "a host removal repeated",
            setup: vec![
                Mutation::AddHost("esx-repeat"),
                Mutation::RemoveHost("esx-repeat"),
            ],
            under_test: Mutation::RemoveHost("esx-repeat"),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "a connector create repeated",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::AddConnector("vcenter.evs.example.com"),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "a connector reconfigured to the appliance it already reaches",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::ReconfigureConnector("vcenter.evs.example.com"),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "a connector removal repeated",
            setup: vec![
                Mutation::AddConnector("vcenter.evs.example.com"),
                Mutation::RemoveConnector,
            ],
            under_test: Mutation::RemoveConnector,
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "an entitlement create repeated",
            setup: vec![
                Mutation::AddConnector("vcenter.evs.example.com"),
                Mutation::AddEntitlements(&["vm-100"]),
            ],
            under_test: Mutation::AddEntitlements(&["vm-100"]),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "an entitlement removal naming a virtual machine that has none",
            setup: vec![Mutation::AddConnector("vcenter.evs.example.com")],
            under_test: Mutation::RemoveEntitlements(&["vm-999"]),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "an Elastic IP association repeated",
            setup: vec![Mutation::AssociateEip("eipalloc-00000000000000003")],
            under_test: Mutation::AssociateEip("eipalloc-00000000000000003"),
            expect_status: 200,
            expect_stamp: false,
        },
        Row {
            case: "an Elastic IP disassociation repeated",
            setup: vec![
                Mutation::AssociateEip("eipalloc-00000000000000004"),
                Mutation::DisassociateEip,
            ],
            under_test: Mutation::DisassociateEip,
            expect_status: 200,
            expect_stamp: false,
        },
        // Nothing changes: the request is REFUSED, so there is no mutation at all.
        Row {
            case: "a removal naming a host the environment does not hold",
            setup: vec![Mutation::AddHost("esx-present")],
            under_test: Mutation::RemoveHost("esx-absent"),
            expect_status: 400,
            expect_stamp: false,
        },
    ]
}

/// The `modifiedAt` this service's own store holds for one environment, in epoch
/// milliseconds — the stamp `GetEnvironment` reports.
fn stored_modified_at(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .map(|environment| environment.modified_at_ms)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"))
}

/// Move one environment's stamp a known span into the past and answer the instant it now
/// carries, so a stamp written from the current clock is telling whatever the clock's
/// resolution.
fn backdate_modified_at(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let backdated = evs::state::now_ms() - BACKDATE_MS;
    let mut store = state.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    environment.modified_at_ms = backdated;
    backdated
}

/// The `modifiedAt` `GetEnvironment` reports for one environment, as the wire carries it.
async fn reported_modified_at(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body["environment"]["modifiedAt"].clone()
}

#[tokio::test]
async fn a_change_to_a_record_an_environment_holds_stamps_the_environment() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // Every row gets its own environment, with a PUBLIC HCX VLAN so the Elastic IP
        // rows have a VLAN to associate with, and settled so no transition the clock owes
        // the environment itself can move the stamp under test.
        let environment = create_public_hcx(&router, &format!("child-stamp-{index}")).await;
        let mut subject = Subject {
            environment,
            connector: None,
            association: None,
        };
        for mutation in &row.setup {
            let (status, body) = apply(&router, case, &mut subject, mutation).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup step {mutation:?} expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
        }
        // Past every transition the setup started, so the only thing that can move the
        // stamp below is the call under test.
        settle(&router);
        let baseline_ms = backdate_modified_at(&router, case, &subject.environment);
        let baseline_wire = reported_modified_at(&router, case, &subject.environment).await;

        let called_at_ms = evs::state::now_ms();
        let (status, body) = apply(&router, case, &mut subject, &row.under_test).await;
        assert_eq!(
            status, row.expect_status,
            "case {case:?}: the call under test expected {}, got {status} ({}) {body}",
            row.expect_status,
            error_type(&body)
        );

        let after_ms = stored_modified_at(&router, case, &subject.environment);
        let after_wire = reported_modified_at(&router, case, &subject.environment).await;
        if row.expect_stamp {
            assert!(
                after_ms >= called_at_ms,
                "case {case:?}: a call that changed a record the environment holds must \
                 stamp modifiedAt from the current clock, expected at least \
                 {called_at_ms}, got {after_ms}"
            );
            assert_ne!(
                after_wire, baseline_wire,
                "case {case:?}: a call that changed a record the environment holds must \
                 move the reported modifiedAt away from {baseline_wire}, got {after_wire}"
            );
        } else {
            assert_eq!(
                after_ms, baseline_ms,
                "case {case:?}: a call that changed nothing must leave modifiedAt at \
                 {baseline_ms}, got {after_ms}"
            );
            assert_eq!(
                after_wire, baseline_wire,
                "case {case:?}: a call that changed nothing must leave the reported \
                 modifiedAt at {baseline_wire}, got {after_wire}"
            );
        }

        // The summary an operation answers with reports the environment AS IT STANDS
        // after the call, so it carries the same stamp the next read of the environment
        // does.
        if !body["environmentSummary"].is_null() {
            assert_eq!(
                body["environmentSummary"]["modifiedAt"], after_wire,
                "case {case:?}: the environmentSummary must report the stamp a later read \
                 reports ({after_wire}), got {}",
                body["environmentSummary"]["modifiedAt"]
            );
        }
    }
}

#[tokio::test]
async fn adding_and_then_deleting_hosts_both_move_the_environment_stamp() {
    // The same rule read off the wire alone, with no store surgery: the host inventory of
    // one environment is changed in both directions, far enough apart in wall-clock time
    // that each stamp would show.
    let router = emulator();
    let environment = create_public_hcx(&router, "child-stamp-wire").await;
    // The create transition is SETTLED by the harness without sleeping for it, which
    // dates the environment at the instant that transition came due — an instant the
    // clock has not reached yet. This test compares real stamps, so it waits for it.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let hosts = ["esx-a", "esx-b", "esx-c", "esx-d"];

    let mut added_at = Value::Null;
    for host_name in hosts {
        let (status, body) = apply(
            &router,
            "adding the hosts",
            &mut Subject {
                environment: environment.clone(),
                connector: None,
                association: None,
            },
            &Mutation::AddHost(host_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case adding {host_name}: expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        added_at = body["environmentSummary"]["modifiedAt"].clone();
    }
    assert_eq!(
        reported_modified_at(&router, "after the adds", &environment).await,
        added_at,
        "case after the adds: GetEnvironment must report the stamp the last add wrote"
    );
    // Far enough apart in wall-clock time that a stamp the deletes write shows.
    tokio::time::sleep(BETWEEN_CALLS).await;

    for host_name in hosts {
        let (status, body) = apply(
            &router,
            "deleting the hosts",
            &mut Subject {
                environment: environment.clone(),
                connector: None,
                association: None,
            },
            &Mutation::RemoveHost(host_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case deleting {host_name}: expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let deleted_at = body["environmentSummary"]["modifiedAt"].clone();
        assert!(
            deleted_at.as_f64().unwrap_or_default() > added_at.as_f64().unwrap_or_default(),
            "case deleting {host_name}: the environment's host inventory changed, so its \
             modifiedAt must move past {added_at}, got {deleted_at}"
        );
        assert_eq!(
            reported_modified_at(&router, "after the delete", &environment).await,
            deleted_at,
            "case deleting {host_name}: GetEnvironment must report the stamp the delete wrote"
        );
    }
}
