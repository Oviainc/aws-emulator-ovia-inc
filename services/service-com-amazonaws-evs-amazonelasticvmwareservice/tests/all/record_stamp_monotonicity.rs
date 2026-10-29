// SPDX-License-Identifier: CC0-1.0

//! What a mutation does to the LAST-MODIFIED stamp of the record it changes, over
//! awsJson1_0.
//!
//! The rule under test: `modifiedAt` is "The date and time that the VLAN was modified"
//! (`smithy_query(query="doc",shape="Vlan",member="modifiedAt",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")`,
//! and the same sentence for `Host` and `Connector`), so a change is dated at the instant
//! it was APPLIED — the instant the store was written, not the instant the request
//! arrived — and for the environment and the VLANs it holds at the LATER of that instant
//! and the stamp the record already carried. Those two records share ONE lifecycle
//! transition, which dates them both at the instant it came due, and a change to a VLAN
//! is a change to the environment that holds it, so neither of the pair a response
//! reports can be dated before the transition they both carry.
//!
//! What the rule generalises over, and so what the tables below span: the KIND of record
//! changed (a VLAN, an ESX host, a connector), the DIRECTION of the change (an
//! association added, an association removed, a host removed, a connector reconfigured,
//! a connector removed), and WHERE the stamp the record already carries sits relative to
//! the call — behind the clock, the ordinary case, or ahead of it, which is what a
//! transition dated at the instant it comes due leaves behind.
//!
//! Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts, and reads each record back through
//! its own lister.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{
    call, emulator, error_type, settle, Emulator, PAST_THE_TRANSITION,
};
use crate::hcx_eip_associations::{create_public_hcx, register_elastic_ip};

/// The connector type every connector here is created with
/// (`smithy_query(query="shape",shape="ConnectorType")` declares it).
const VCENTER: &str = "VCENTER";

/// The credential secret every connector here is created with.
const SECRET_IDENTIFIER: &str = "evs/vcf-appliance-credentials";

/// The appliance a connector reaches before and after it is reconfigured.
const APPLIANCE: &str = "vcenter.evs.example.com";
const MOVED_APPLIANCE: &str = "vcenter-moved.evs.example.com";

/// The instance type every host here runs on, and the key pair it is launched with.
const INSTANCE_TYPE: &str = "i4i.metal";
const KEY_NAME: &str = "evs-key";

/// The `VlanState` an environment's create transition moves its VLANs to
/// (`smithy_query(query="shape",shape="VlanState")` declares it).
const CREATED: &str = "CREATED";

/// Which record of an environment a row changes.
#[derive(Debug, Clone)]
enum Record {
    /// The environment's public HCX VLAN.
    HcxVlan,
    /// The ESX host of that `hostName`.
    Host(String),
    /// The connector of that `connectorId`.
    Connector(String),
}

/// The one call under test of a row.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// `AssociateEipToVlan`: associate the row's allocation with the public HCX VLAN.
    AssociateEip,
    /// `DisassociateEipFromVlan`: remove the association the setup recorded.
    DisassociateEip,
    /// `DeleteEnvironmentHost`: remove the ESX host the setup added.
    RemoveHost,
    /// `UpdateEnvironmentConnector`: point the connector at another appliance.
    ReconfigureConnector,
    /// `DeleteEnvironmentConnector`: remove the connector the setup added.
    RemoveConnector,
}

/// The environment a row runs against, the record under test, and the ids its call
/// addresses.
struct Subject {
    environment: String,
    record: Record,
    /// The Elastic IP allocation an associate row names — one per row, because an
    /// allocation carries one association at a time across the whole account.
    allocation: Option<String>,
    /// The association a disassociate row removes, as its setup recorded it.
    association: Option<String>,
}

/// Create the environment a row runs against and the record its call under test changes.
async fn prepare(router: &Emulator, case: &str, index: usize, change: Change) -> Subject {
    // A PUBLIC HCX VLAN, because the Elastic IP rows need one to associate with, and
    // settled so every lister this test reads a record back through answers.
    let environment = create_public_hcx(router, &format!("stamp-{index}")).await;
    // One allocation per row: an Elastic IP address carries one association at a time, so
    // two rows cannot name the same one.
    let allocation_id = format!("eipalloc-{index:017x}");
    match change {
        Change::AssociateEip => {
            // The allocation has to be one the account's own Amazon EC2 store holds.
            register_elastic_ip(&router.engine, &allocation_id, Some("198.51.100.7"));
            Subject {
                environment,
                record: Record::HcxVlan,
                allocation: Some(allocation_id),
                association: None,
            }
        }
        Change::DisassociateEip => {
            register_elastic_ip(&router.engine, &allocation_id, Some("198.51.100.8"));
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
            assert_eq!(
                status, 200,
                "case {case:?}: the setup associate expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let association = body["vlan"]["eipAssociations"]
                .as_array()
                .and_then(|associations| associations.first())
                .and_then(|association| association["associationId"].as_str())
                .unwrap_or_else(|| panic!("case {case:?}: no associationId in {body}"))
                .to_string();
            Subject {
                environment,
                record: Record::HcxVlan,
                allocation: Some(allocation_id),
                association: Some(association),
            }
        }
        Change::RemoveHost => {
            // "DNS hostnames for hosts must be unique across Amazon EVS environments"
            // (`HostInfoForCreate$hostName` doc), so each row names its own.
            let host_name = format!("esx-stamp-{index}");
            let (status, body) = call(
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
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the setup host create expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            Subject {
                environment,
                record: Record::Host(host_name),
                allocation: None,
                association: None,
            }
        }
        Change::ReconfigureConnector | Change::RemoveConnector => {
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
                "case {case:?}: the setup connector create expected 200, got {status} ({}) {body}",
                error_type(&body)
            );
            let connector = body["connector"]["connectorId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no connectorId in {body}"))
                .to_string();
            Subject {
                environment,
                record: Record::Connector(connector),
                allocation: None,
                association: None,
            }
        }
    }
}

/// Drive the row's one call under test.
async fn apply(router: &Emulator, case: &str, subject: &Subject, change: Change) -> (u16, Value) {
    let environment = subject.environment.clone();
    match change {
        Change::AssociateEip => {
            let allocation = subject
                .allocation
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: the row registered no allocation"));
            call(
                router,
                "AssociateEipToVlan",
                &json!({
                    "environmentId": environment,
                    "vlanName": "hcx",
                    "allocationId": allocation,
                }),
            )
            .await
        }
        Change::DisassociateEip => {
            let association = subject
                .association
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: the setup recorded no association"));
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
        Change::RemoveHost => {
            let Record::Host(host_name) = &subject.record else {
                panic!("case {case:?}: the row's record is no host");
            };
            call(
                router,
                "DeleteEnvironmentHost",
                &json!({ "environmentId": environment, "hostName": host_name }),
            )
            .await
        }
        Change::ReconfigureConnector => {
            let connector = connector_of(case, subject);
            call(
                router,
                "UpdateEnvironmentConnector",
                &json!({
                    "environmentId": environment,
                    "connectorId": connector,
                    "applianceFqdn": MOVED_APPLIANCE,
                }),
            )
            .await
        }
        Change::RemoveConnector => {
            let connector = connector_of(case, subject);
            call(
                router,
                "DeleteEnvironmentConnector",
                &json!({ "environmentId": environment, "connectorId": connector }),
            )
            .await
        }
    }
}

/// The connector a connector-addressed row names, as its setup minted it.
fn connector_of(case: &str, subject: &Subject) -> String {
    match &subject.record {
        Record::Connector(connector_id) => connector_id.clone(),
        other => panic!("case {case:?}: the row's record is no connector: {other:?}"),
    }
}

/// The output member the call under test reports the record under test through.
fn reported_member(change: Change) -> &'static str {
    match change {
        Change::AssociateEip | Change::DisassociateEip => "vlan",
        Change::RemoveHost => "host",
        Change::ReconfigureConnector | Change::RemoveConnector => "connector",
    }
}

/// The `modifiedAt` this service's own store holds for the record under test, in epoch
/// milliseconds.
fn stored_stamp(router: &Emulator, case: &str, subject: &Subject) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(&subject.environment)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {}", subject.environment));
    match &subject.record {
        Record::HcxVlan => environment
            .vlans
            .iter()
            .find(|vlan| vlan.function_name == "hcx")
            .map(|vlan| vlan.modified_at_ms)
            .unwrap_or_else(|| panic!("case {case:?}: the environment holds an hcx VLAN")),
        Record::Host(host_name) => environment
            .hosts
            .get(host_name)
            .map(|host| host.modified_at_ms)
            .unwrap_or_else(|| panic!("case {case:?}: the environment holds host {host_name}")),
        Record::Connector(connector_id) => environment
            .connectors
            .get(connector_id)
            .map(|connector| connector.modified_at_ms)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the environment holds connector {connector_id}")
            }),
    }
}

/// The `modifiedAt` the record under test's own lister reports, as the wire carries it.
async fn listed_stamp(router: &Emulator, case: &str, subject: &Subject) -> Value {
    let (operation, items, key, wanted) = match &subject.record {
        Record::HcxVlan => (
            "ListEnvironmentVlans",
            "environmentVlans",
            "functionName",
            "hcx".to_string(),
        ),
        Record::Host(host_name) => (
            "ListEnvironmentHosts",
            "environmentHosts",
            "hostName",
            host_name.clone(),
        ),
        Record::Connector(connector_id) => (
            "ListEnvironmentConnectors",
            "connectors",
            "connectorId",
            connector_id.clone(),
        ),
    };
    let (status, body) = call(
        router,
        operation,
        &json!({ "environmentId": subject.environment }),
    )
    .await;
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
        .map(|item| item["modifiedAt"].clone())
        .unwrap_or_else(|| panic!("case {case:?}: no {key} {wanted} in {body}"))
}

/// A `modifiedAt` the wire carries (epoch seconds) in the epoch milliseconds the store
/// holds it as.
fn wire_ms(case: &str, label: &str, value: &Value) -> i64 {
    let seconds = value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {label} carries no modifiedAt, got {value}"));
    (seconds * 1000.0).round() as i64
}

/// The `modifiedAt` `GetEnvironment` reports for one environment, in epoch milliseconds.
async fn environment_stamp(router: &Emulator, case: &str, environment_id: &str) -> i64 {
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
    wire_ms(case, "environment", &body["environment"]["modifiedAt"])
}

/// How far AHEAD of the clock a transition's instant is placed before the call under
/// test, so the assertions below do not depend on how long the calls before them took —
/// a minute, far longer than a test takes.
const AHEAD_MS: i64 = 60_000;

/// Date one environment and the VLANs it holds at a known instant ahead of the clock and
/// answer that instant.
///
/// What a lifecycle transition leaves behind: it moves the environment and every VLAN of
/// it together and dates them both at the instant it came due, which is an instant the
/// clock has not reached when the call that materialises it evaluates a change.
fn postdate_environment_and_vlans(router: &Emulator, case: &str, environment_id: &str) -> i64 {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let due_at_ms = evs::state::now_ms() + AHEAD_MS;
    let mut store = state.lock();
    let environment = store
        .environments
        .get_mut(environment_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds {environment_id}"));
    environment.modified_at_ms = due_at_ms;
    for vlan in &mut environment.vlans {
        vlan.modified_at_ms = due_at_ms;
    }
    due_at_ms
}

#[tokio::test]
async fn a_change_dates_the_record_at_the_instant_it_was_applied() {
    // Each row is one kind of record and one direction of change, with every transition
    // the record rode already behind the clock: the ordinary case, in which the change
    // itself is the later instant and so the one the record is dated at.
    let rows: [(&str, Change); 5] = [
        (
            "an Elastic IP associated with the public HCX VLAN",
            Change::AssociateEip,
        ),
        (
            "an Elastic IP disassociated from the public HCX VLAN",
            Change::DisassociateEip,
        ),
        ("an ESX host removed", Change::RemoveHost),
        (
            "a connector pointed at another appliance",
            Change::ReconfigureConnector,
        ),
        ("a connector removed", Change::RemoveConnector),
    ];

    let router = emulator();
    for (index, (case, change)) in rows.into_iter().enumerate() {
        let subject = prepare(&router, case, index, change).await;
        // Wait every transition the setup started out, so the instants they are dated at
        // are behind the clock, and then materialise them.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        settle(&router);

        let before_ms = stored_stamp(&router, case, &subject);
        let before_wire = listed_stamp(&router, case, &subject).await;
        assert_eq!(
            wire_ms(case, "the stamp before the call", &before_wire),
            before_ms,
            "case {case:?}: the lister must report the stamp the store holds ({before_ms}), \
             got {before_wire}"
        );

        let called_at_ms = evs::state::now_ms();
        assert!(
            before_ms <= called_at_ms,
            "case {case:?}: this row needs a record dated behind the clock \
             ({called_at_ms}), got {before_ms}"
        );
        let (status, body) = apply(&router, case, &subject, change).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call under test expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let returned_at_ms = evs::state::now_ms();
        let after_ms = stored_stamp(&router, case, &subject);

        // Dated at the instant the change was APPLIED, which lies between the instant
        // before the call and the instant it answered — and so past the stamp the record
        // carried before it.
        assert!(
            after_ms >= called_at_ms && after_ms <= returned_at_ms,
            "case {case:?}: a change must date the record at the instant it was applied, \
             expected between {called_at_ms} and {returned_at_ms}, got {after_ms}"
        );
        assert!(
            after_ms > before_ms,
            "case {case:?}: a change must move modifiedAt on from {before_ms}, got \
             {after_ms}"
        );

        // The response reports the stamp the store now holds, and a later read of the
        // record reports one no earlier than it: only a transition the clock owes can
        // move it on from there.
        let member = reported_member(change);
        let reported = body[member]["modifiedAt"].clone();
        assert_eq!(
            wire_ms(case, member, &reported),
            after_ms,
            "case {case:?}: the {member} answered must carry the stamp the store holds \
             ({after_ms}), got {reported}"
        );
        let listed = listed_stamp(&router, case, &subject).await;
        assert!(
            wire_ms(case, "the stamp read back", &listed) >= after_ms,
            "case {case:?}: a later read must not report a stamp earlier than {after_ms}, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn a_vlan_change_is_dated_at_the_later_of_the_change_and_the_transition_it_rides() {
    // The ordering the rule exists for: the environment's create transition dates the
    // environment AND every VLAN of it at the instant it came due, an instant the clock
    // has not reached yet, so a change to the VLAN evaluated before that instant must be
    // dated at the transition's instant rather than before the state change the same
    // response reports.
    let rows: [(&str, Change); 2] = [
        (
            "an Elastic IP associated with a VLAN the create transition just stamped",
            Change::AssociateEip,
        ),
        (
            "an Elastic IP disassociated from a VLAN the create transition just stamped",
            Change::DisassociateEip,
        ),
    ];

    let router = emulator();
    for (index, (case, change)) in rows.into_iter().enumerate() {
        let subject = prepare(&router, case, index, change).await;
        // Past the create transition, so the VLAN is in the state that transition moved it
        // to and no transition is left that could stamp it again.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        settle(&router);
        // The transition's own instant, dated ahead of the clock exactly as a
        // materialisation dates it.
        let became_created_ms = postdate_environment_and_vlans(&router, case, &subject.environment);
        let called_at_ms = evs::state::now_ms();
        assert!(
            became_created_ms > called_at_ms,
            "case {case:?}: this row needs a VLAN dated ahead of the clock \
             ({called_at_ms}), got {became_created_ms}"
        );

        let (status, body) = apply(&router, case, &subject, change).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call under test expected 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let after_ms = stored_stamp(&router, case, &subject);
        assert_eq!(
            after_ms, became_created_ms,
            "case {case:?}: the later of the change and the transition the VLAN rides is \
             the transition's own instant {became_created_ms}, got {after_ms}"
        );

        // The VLAN the response reports carries the state that transition moved it to, so
        // its stamp cannot be earlier than the instant it reached that state.
        let vlan = body["vlan"].clone();
        assert_eq!(
            vlan["vlanState"],
            json!(CREATED),
            "case {case:?}: the response reports the VLAN as it stands, expected \
             {CREATED}, got {}",
            vlan["vlanState"]
        );
        let dated_ms = wire_ms(case, "vlan", &vlan["modifiedAt"]);
        assert!(
            dated_ms >= became_created_ms,
            "case {case:?}: a VLAN reported in {CREATED} cannot be dated before the \
             instant it became {CREATED} ({became_created_ms}), got {dated_ms}"
        );

        // And it is dated no earlier than the environment that holds it: the change is a
        // change to both records, and both take the later of the change and the transition
        // they share.
        let environment_ms = environment_stamp(&router, case, &subject.environment).await;
        assert!(
            dated_ms >= environment_ms,
            "case {case:?}: the VLAN carries the change the environment was re-stamped \
             for, so it cannot be dated before the environment ({environment_ms}), got \
             {dated_ms}"
        );

        // Persisted: the VLAN's own lister reports the same stamp.
        let listed = listed_stamp(&router, case, &subject).await;
        assert_eq!(
            wire_ms(case, "the stamp read back", &listed),
            after_ms,
            "case {case:?}: the lister must report the stamp the store holds ({after_ms}), \
             got {listed}"
        );
    }
}
