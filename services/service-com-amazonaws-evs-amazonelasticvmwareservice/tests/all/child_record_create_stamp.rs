// SPDX-License-Identifier: CC0-1.0

//! When a call that ADDS or REMOVES a record an environment holds dates that record, over
//! awsJson1_0.
//!
//! The rule under test: `createdAt` is "The date and time that the host was created" and
//! `modifiedAt` "The date and time that the host was modified"
//! (`smithy_query(query="doc",shape="Host",member="createdAt",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`,
//! and the same sentences for `Connector`; `VmEntitlement$stoppedAt` is "The date and time
//! that the entitlement stopped"), so the stamp a call writes is the instant its change
//! was APPLIED to the store — not the instant the request arrived. A call therefore reads
//! that instant with the store lock HELD and after the transitions the clock already owed
//! have been materialised, which makes the stamp no earlier than any stamp that
//! materialisation wrote and keeps the re-stamp of the environment holding the record from
//! being dropped by the guard that stops a stamp going backwards.
//!
//! Each row drives the ordering the rule exists for, and drives it deterministically: the
//! test takes this service's store lock itself, issues the call under test while holding
//! it, keeps it until past the instant a transition in flight is due, and only then
//! releases it. The call therefore arrives BEFORE that transition is due and is applied
//! AFTER it, which is the window in which an instant read on the request's way in would be
//! earlier than the transition the call itself materialises.
//!
//! What the rule generalises over, and so what the table spans: the KIND of record written
//! (an ESX host, a connector, an entitlement), the DIRECTION of the change (a record added
//! and a record removed), the STAMP the call writes (a new record's `createdAt` and
//! `modifiedAt`, a stopped record's `stoppedAt`, and the environment's own `modifiedAt` for
//! the change to its inventory), and the transition waited through (the environment's own
//! lifecycle transition, and a connector's and an entitlement's own status change).
//! `tests/all/record_stamp_backwards_guard.rs` owns the same rule for the calls that MUTATE
//! a record that already exists.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads each record back through
//! its own reader.

use std::sync::Arc;
use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator, PAST_THE_TRANSITION};
// The suite that owns this same rule for the calls that MUTATE a record that already
// exists holds the span a row keeps the store lock for, the due-instant arithmetic and the
// readers both halves need, so they are shared rather than written a second time here.
use crate::record_stamp_backwards_guard::{
    due_from, environment_stamp, held_for, member, wire_ms,
};

/// The connector type every connector here is created with, and the one an entitlement is
/// tracked through (`smithy_query(query="shape",shape="ConnectorType")` declares it).
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape",shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The credential secret every connector here is created with.
const SECRET_IDENTIFIER: &str = "evs/vcf-appliance-credentials";

/// The appliance every connector here reaches.
const APPLIANCE: &str = "vcenter.evs.example.com";

/// The instance type every host here runs on, and the key pair it is launched with.
const INSTANCE_TYPE: &str = "i4i.metal";
const KEY_NAME: &str = "evs-key";

/// The virtual machine the entitlement rows entitle and stop.
const VM_ID: &str = "vm-700";

/// The one call under test of a row.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// `CreateEnvironmentHost`: add an ESX host to the environment.
    AddHost,
    /// `CreateEnvironmentConnector`: add a vCenter connector to the environment.
    AddConnector,
    /// `CreateEntitlement`: entitle one virtual machine through the connector.
    AddEntitlement,
    /// `DeleteEntitlement`: stop tracking the virtual machine the setup entitled.
    RemoveEntitlement,
}

/// The environment a row runs against, the ids its call under test addresses, and when the
/// transition in flight as that call arrives comes due.
struct Subject {
    environment: String,
    /// The connector the entitlement rows are tracked through, as the setup minted it.
    connector: Option<String>,
    /// The DNS hostname the host row adds — "DNS hostnames for hosts must be unique across
    /// Amazon EVS environments" (`HostInfoForCreate$hostName` doc), so each row names its
    /// own.
    host_name: String,
    /// The epoch milliseconds the transition the call under test will materialise is due
    /// at: the instant the last record the setup created plus the span this service owes a
    /// transition in. The call has to arrive before it for the window to be the one the
    /// rule is about.
    due_at_ms: i64,
}

/// Create the environment a row runs against and whatever record its call under test
/// addresses, leaving a transition STILL PENDING.
async fn prepare(router: &Emulator, case: &str, index: usize, change: Change) -> Subject {
    let (status, body) = call(
        router,
        "CreateEnvironment",
        &router.create_request(Some(&format!("create-stamp-{index}"))),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup create expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    let environment = member(case, &body["environment"], "environmentId");
    let environment_created_ms = wire_ms(case, "the environment", &body["environment"]["createdAt"]);
    let host_name = format!("esx-create-stamp-{index}");

    match change {
        // The transition in flight is the environment's own `CREATING` one, so nothing is
        // waited out: the call under test is what materialises it — which is also what
        // makes the environment deployed enough to take a host.
        Change::AddHost | Change::AddConnector => Subject {
            environment,
            connector: None,
            host_name,
            due_at_ms: due_from(environment_created_ms),
        },
        Change::AddEntitlement | Change::RemoveEntitlement => {
            // The connector an entitlement is tracked through is added to an environment
            // that is past initialization, so the environment's own transition is waited
            // out first and the transition left in flight is the connector's own.
            tokio::time::sleep(PAST_THE_TRANSITION).await;
            let (status, body) = call(
                router,
                "CreateEnvironmentConnector",
                &json!({
                    "environmentId": environment,
                    "type": VCENTER,
                    "applianceFqdn": APPLIANCE,
                    "secretIdentifier": SECRET_IDENTIFIER,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup connector create expected 200, got {status} ({}) \
                 {body}",
                error_type(&body)
            );
            let connector = member(case, &body["connector"], "connectorId");
            let connector_created_ms = wire_ms(case, "the connector", &body["connector"]["createdAt"]);
            if matches!(change, Change::RemoveEntitlement) {
                // The record the removal stops, left with the validation the clock owes it
                // still in flight — a second transition the call under test materialises.
                let (status, body) = call(
                    router,
                    "CreateEntitlement",
                    &json!({
                        "environmentId": environment,
                        "connectorId": connector,
                        "entitlementType": WINDOWS_SERVER,
                        "vmIds": [VM_ID],
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup entitlement create expected 200, got {status} \
                     ({}) {body}",
                    error_type(&body)
                );
            }
            Subject {
                environment,
                connector: Some(connector),
                host_name,
                due_at_ms: due_from(connector_created_ms),
            }
        }
    }
}

/// The row's one call under test, as the operation it targets and the body it sends.
fn request(case: &str, subject: &Subject, change: Change) -> (&'static str, Value) {
    let environment = subject.environment.clone();
    match change {
        Change::AddHost => (
            "CreateEnvironmentHost",
            json!({
                "environmentId": environment,
                "host": {
                    "hostName": subject.host_name,
                    "keyName": KEY_NAME,
                    "instanceType": INSTANCE_TYPE,
                },
            }),
        ),
        Change::AddConnector => (
            "CreateEnvironmentConnector",
            json!({
                "environmentId": environment,
                "type": VCENTER,
                "applianceFqdn": APPLIANCE,
                "secretIdentifier": SECRET_IDENTIFIER,
            }),
        ),
        Change::AddEntitlement => (
            "CreateEntitlement",
            json!({
                "environmentId": environment,
                "connectorId": connector_of(case, subject),
                "entitlementType": WINDOWS_SERVER,
                "vmIds": [VM_ID],
            }),
        ),
        Change::RemoveEntitlement => (
            "DeleteEntitlement",
            json!({
                "environmentId": environment,
                "connectorId": connector_of(case, subject),
                "entitlementType": WINDOWS_SERVER,
                "vmIds": [VM_ID],
            }),
        ),
    }
}

/// The connector an entitlement row is tracked through, as its setup minted it.
fn connector_of(case: &str, subject: &Subject) -> String {
    subject
        .connector
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the row minted no connector"))
}

/// The ONE instant the call under test dated its change at, as the store holds it — and,
/// for a record the call stores, the stamp the response reports for it, which has to be
/// that same instant.
///
/// A new host and a new connector carry it as BOTH `createdAt` and `modifiedAt`: a record
/// that has just been created has not been modified since. An entitlement carries it as
/// its stored creation instant only, because the modelled `VmEntitlement` declares no
/// `createdAt` member at all
/// (`smithy_query(query="shape",shape="VmEntitlement")`), and a STOPPED entitlement is
/// reported rather than stored, so its instant is read off the response's `stoppedAt`.
fn applied_stamp(router: &Emulator, case: &str, subject: &Subject, change: Change, body: &Value) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(&subject.environment)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {}", subject.environment));
    match change {
        Change::AddHost => {
            let host = environment
                .hosts
                .get(&subject.host_name)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the environment holds host {}", subject.host_name)
                });
            assert_eq!(
                host.modified_at_ms, host.created_at_ms,
                "case {case:?}: a host that has just been created carries its creation \
                 instant ({}) as modifiedAt too, got {}",
                host.created_at_ms, host.modified_at_ms
            );
            assert_reported_pair(case, "host", &body["host"], host.created_at_ms);
            host.created_at_ms
        }
        Change::AddConnector => {
            let connector_id = member(case, &body["connector"], "connectorId");
            let connector = environment
                .connectors
                .get(&connector_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the environment holds connector {connector_id}")
                });
            assert_eq!(
                connector.modified_at_ms, connector.created_at_ms,
                "case {case:?}: a connector that has just been created carries its creation \
                 instant ({}) as modifiedAt too, got {}",
                connector.created_at_ms, connector.modified_at_ms
            );
            assert_reported_pair(case, "connector", &body["connector"], connector.created_at_ms);
            connector.created_at_ms
        }
        Change::AddEntitlement => environment
            .entitlements
            .values()
            .find(|entitlement| entitlement.vm_id == VM_ID)
            .map(|entitlement| entitlement.created_at_ms)
            .unwrap_or_else(|| panic!("case {case:?}: the environment holds an entitlement of {VM_ID}")),
        Change::RemoveEntitlement => {
            let stopped = body["entitlements"]
                .as_array()
                .and_then(|entitlements| entitlements.first())
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no stopped entitlement in {body}"));
            let stopped_ms = wire_ms(case, "the stopped entitlement", &stopped["stoppedAt"]);
            // A record cannot stop tracking before it started: `startedAt` is the instant
            // the validation this very call materialised came due.
            if !stopped["startedAt"].is_null() {
                let started_ms = wire_ms(case, "the stopped entitlement", &stopped["startedAt"]);
                assert!(
                    stopped_ms >= started_ms,
                    "case {case:?}: an entitlement cannot stop before it started \
                     ({started_ms}), got {stopped_ms}"
                );
            }
            stopped_ms
        }
    }
}

/// The record a create reports carries one instant as both of its stamps, and that instant
/// is the one the store holds.
fn assert_reported_pair(case: &str, label: &str, record: &Value, stored_ms: i64) {
    let created_ms = wire_ms(case, label, &record["createdAt"]);
    let modified_ms = wire_ms(case, label, &record["modifiedAt"]);
    assert_eq!(
        created_ms, stored_ms,
        "case {case:?}: the {label} answered must carry the creation instant the store \
         holds ({stored_ms}), got {created_ms}"
    );
    assert_eq!(
        modified_ms, stored_ms,
        "case {case:?}: the {label} answered must carry {stored_ms} as modifiedAt too, got \
         {modified_ms}"
    );
}

/// Every `modifiedAt` and transition-written stamp this service's store holds for one
/// environment and the records it holds, as `(what the stamp belongs to, the stamp)`.
///
/// What the call under test's own stamp is checked against: a materialisation writes the
/// instant a transition came DUE onto the records that rode it, so a stamp this call wrote
/// cannot be earlier than any of these.
fn stamps_after(router: &Emulator, case: &str, environment_id: &str) -> Vec<(String, i64)> {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    let mut stamps = vec![("the environment".to_string(), environment.modified_at_ms)];
    for vlan in &environment.vlans {
        stamps.push((format!("VLAN {}", vlan.function_name), vlan.modified_at_ms));
    }
    for host in environment.hosts.values() {
        stamps.push((format!("host {}", host.host_name), host.modified_at_ms));
    }
    for connector in environment.connectors.values() {
        stamps.push((
            format!("connector {}", connector.connector_id),
            connector.modified_at_ms,
        ));
    }
    for entitlement in environment.entitlements.values() {
        stamps.push((
            format!("entitlement {} createdAt", entitlement.vm_id),
            entitlement.created_at_ms,
        ));
        if let Some(started_at_ms) = entitlement.started_at_ms {
            stamps.push((
                format!("entitlement {} startedAt", entitlement.vm_id),
                started_at_ms,
            ));
        }
    }
    stamps
}

/// The `modifiedAt` `GetEnvironment` reports for one environment, in epoch milliseconds.
async fn reported_environment_stamp(router: &Emulator, case: &str, environment_id: &str) -> i64 {
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
    wire_ms(case, "the environment", &body["environment"]["modifiedAt"])
}

/// Read the record the call under test wrote back through its OWN reader and assert what
/// that reader reports about it: for a record the call stored, the stamp the store holds;
/// for the entitlement the removal stopped, that it is no longer reported at all.
async fn assert_read_back(
    router: &Emulator,
    case: &str,
    subject: &Subject,
    change: Change,
    applied_ms: i64,
) {
    match change {
        Change::AddHost => {
            let host = listed(
                router,
                case,
                &json!({ "environmentId": subject.environment }),
                "ListEnvironmentHosts",
                "environmentHosts",
                "hostName",
                &subject.host_name,
            )
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: ListEnvironmentHosts must report host {}",
                    subject.host_name
                )
            });
            assert_stamps_read_back(case, "host", &host, applied_ms);
        }
        Change::AddConnector => {
            // The row's call is the second connector create of the environment only for
            // the entitlement rows, so the one this reader has to find is the VCENTER one.
            let connector = listed(
                router,
                case,
                &json!({ "environmentId": subject.environment }),
                "ListEnvironmentConnectors",
                "connectors",
                "type",
                VCENTER,
            )
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListEnvironmentConnectors must report the connector")
            });
            assert_stamps_read_back(case, "connector", &connector, applied_ms);
        }
        Change::AddEntitlement => {
            let entitlement = listed(
                router,
                case,
                &json!({
                    "environmentId": subject.environment,
                    "connectorId": connector_of(case, subject),
                    "entitlementType": WINDOWS_SERVER,
                }),
                "ListVmEntitlements",
                "entitlements",
                "vmId",
                VM_ID,
            )
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListVmEntitlements must report the entitlement of {VM_ID}")
            });
            // The record is reported as the create left it: still being validated, so it
            // has not started tracking and carries no `startedAt` the stamp could fall
            // behind.
            assert_eq!(
                entitlement["status"],
                json!("CREATING"),
                "case {case:?}: the entitlement read back must report CREATING, got {entitlement}"
            );
        }
        Change::RemoveEntitlement => {
            let still_there = listed(
                router,
                case,
                &json!({
                    "environmentId": subject.environment,
                    "connectorId": connector_of(case, subject),
                    "entitlementType": WINDOWS_SERVER,
                }),
                "ListVmEntitlements",
                "entitlements",
                "vmId",
                VM_ID,
            )
            .await;
            assert!(
                still_there.is_none(),
                "case {case:?}: ListVmEntitlements must report no entitlement of {VM_ID} once \
                 it is stopped, got {still_there:?}"
            );
        }
    }
}

/// What a record's own reader reports about the stamps the call under test wrote: the
/// creation instant it was dated at, which nothing moves, and a `modifiedAt` no earlier
/// than it — only the status change the clock owes the new record can move that one on.
fn assert_stamps_read_back(case: &str, label: &str, record: &Value, applied_ms: i64) {
    let created_ms = wire_ms(case, label, &record["createdAt"]);
    assert_eq!(
        created_ms, applied_ms,
        "case {case:?}: the {label}'s own reader must report the creation instant the store \
         holds ({applied_ms}), got {created_ms}"
    );
    let modified_ms = wire_ms(case, label, &record["modifiedAt"]);
    assert!(
        modified_ms >= applied_ms,
        "case {case:?}: the {label}'s own reader must not report a modifiedAt earlier than \
         {applied_ms}, got {modified_ms}"
    );
}

/// The item one lister reports under `key` = `wanted`, or `None` when it reports none.
async fn listed(
    router: &Emulator,
    case: &str,
    request_body: &Value,
    operation: &str,
    items: &str,
    key: &str,
    wanted: &str,
) -> Option<Value> {
    let (status, body) = call(router, operation, request_body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} ({}) {body}",
        error_type(&body)
    );
    body[items]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {items} in {body}"))
        .iter()
        .find(|item| item[key] == json!(wanted))
        .cloned()
}

// Two worker threads: the call under test BLOCKS one of them on this service's store lock
// while the test holds it, so a second is needed to run the test's own task on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_a_call_writes_is_dated_after_the_transition_the_same_call_materialised() {
    let rows: [(&str, Change); 4] = [
        ("an ESX host added", Change::AddHost),
        ("a connector added", Change::AddConnector),
        ("a virtual machine entitled", Change::AddEntitlement),
        ("an entitlement stopped", Change::RemoveEntitlement),
    ];

    for (index, (case, change)) in rows.into_iter().enumerate() {
        let router = Arc::new(emulator());
        let subject = prepare(&router, case, index, change).await;
        let (operation, body) = request(case, &subject, change);

        // This service's store lock, taken and HELD past the instant the transitions in
        // flight are due, on a thread of its own so the call under test — issued below
        // while it is held — cannot be applied until it is released. The thread SIGNALS
        // once it holds the lock, so the call is issued against a lock that is provably
        // held rather than after a span assumed to be long enough. Answers the instant it
        // was released at.
        let (held, taken) = ::tokio::sync::oneshot::channel();
        let holding = tokio::task::spawn_blocking({
            let engine = Arc::clone(&router.engine);
            move || {
                let state = evs::state::service_state(&*engine)
                    .expect("the in-memory engine carries this service's store");
                let store = state.lock();
                let _ = held.send(());
                std::thread::sleep(held_for());
                let released_at_ms = evs::state::now_ms();
                drop(store);
                released_at_ms
            }
        });
        taken.await.expect("the store lock was taken");

        let arrived_at_ms = evs::state::now_ms();
        assert!(
            arrived_at_ms < subject.due_at_ms,
            "case {case:?}: the call under test must arrive before the transition in flight \
             is due ({}), got {arrived_at_ms}",
            subject.due_at_ms
        );
        let waiting = tokio::spawn({
            let router = Arc::clone(&router);
            async move { call(&router, operation, &body).await }
        });

        let released_at_ms = holding.await.expect("the lock was held and released");
        let (status, answered) = waiting.await.expect("the call under test ran to completion");
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} expected 200, got {status} ({}) {answered}",
            error_type(&answered)
        );

        // Dated at the instant the change was APPLIED, which cannot be before the lock was
        // released — and so not at the instant the request arrived at.
        let applied_ms = applied_stamp(&router, case, &subject, change, &answered);
        assert!(
            applied_ms >= released_at_ms,
            "case {case:?}: the change must be dated at the instant it was applied, which is \
             no earlier than {released_at_ms} (the request arrived at {arrived_at_ms}), got \
             {applied_ms}"
        );

        // And so after every stamp the materialisation inside the same call wrote: the
        // environment's and its VLANs' for the lifecycle transition, a connector's for its
        // own state change, an entitlement's for the validation that started its tracking.
        for (what, stamp) in stamps_after(&router, case, &subject.environment) {
            assert!(
                applied_ms >= stamp,
                "case {case:?}: the change must not be dated before {what} ({stamp}), which \
                 the same call materialised, got {applied_ms}"
            );
        }

        // The environment's inventory changed, so the environment is re-stamped at that
        // very instant rather than left at the instant the transition the call materialised
        // was due.
        let environment_ms = environment_stamp(&router, case, &subject.environment);
        assert_eq!(
            environment_ms, applied_ms,
            "case {case:?}: a change to a record the environment holds must re-stamp the \
             environment at the instant it was applied ({applied_ms}), got {environment_ms}"
        );
        assert!(
            environment_ms > subject.due_at_ms,
            "case {case:?}: the environment's stamp must move past the instant the \
             transition the call materialised was due ({}), got {environment_ms}",
            subject.due_at_ms
        );
        // The summary a create answers with reports the environment AS IT STANDS after the
        // call, so it carries that same stamp — and so does a later read.
        if !answered["environmentSummary"].is_null() {
            let summary_ms = wire_ms(
                case,
                "the environmentSummary",
                &answered["environmentSummary"]["modifiedAt"],
            );
            assert_eq!(
                summary_ms, environment_ms,
                "case {case:?}: the environmentSummary answered must carry the stamp the \
                 store holds ({environment_ms}), got {summary_ms}"
            );
        }
        let read_back_ms = reported_environment_stamp(&router, case, &subject.environment).await;
        assert_eq!(
            read_back_ms, environment_ms,
            "case {case:?}: GetEnvironment must report the stamp the store holds \
             ({environment_ms}), got {read_back_ms}"
        );

        // Persisted: the record the call wrote reads back through its own reader.
        assert_read_back(&router, case, &subject, change, applied_ms).await;
    }
}
