// SPDX-License-Identifier: CC0-1.0

//! What an environment's deletion takes with it, end to end over awsJson1_0.
//!
//! The rule under test: deleting an Amazon EVS environment deletes the resources that
//! BELONG to that environment, so once the deletion is accepted none of them is live any
//! more. Where it comes from:
//!
//! * "Environment deletion also deletes the associated Amazon EVS VLAN subnets and
//!   Amazon Web Services Secrets Manager secrets that Amazon EVS created"
//!   (`DeleteEnvironment` doc) — the environment's own resources go away with it, and
//!   `tests/all/vlan_subnet_propagation.rs` owns the VLAN half of that sentence.
//! * A connector and a VM entitlement are resources OF one environment
//!   (`Connector$environmentId`, `VmEntitlement$environmentId`,
//!   `smithy_query(query="shape", shape="Connector,VmEntitlement")`).
//! * `ConnectorState` declares `DELETING` and `DELETED` and `EntitlementStatus` declares
//!   `DELETED` (`smithy_query(query="shape",
//!   shape="ConnectorState,EntitlementStatus")`), which are the terminal values those
//!   records reach: a connector's connection to its appliance is torn down, so it is
//!   `DELETING` while the environment is and `DELETED` when the environment is, while an
//!   entitlement's status enum declares nothing between tracking and stopped, so its
//!   usage tracking stops at once and `stoppedAt` is stamped.
//! * An entitlement is read through the connector it was created through
//!   (`ListVmEntitlements`), and a connector that is being torn down resolves to nothing
//!   ("The resource might not be specified correctly, or it may have a `state` of
//!   `DELETED`", `ResourceNotFoundException` doc;
//!   `tests/all/list_vm_entitlements_connector.rs` owns that rule) — so the entitlements
//!   of a deleted environment are no longer listable through it.
//!
//! The harness (`emulator`, `call`, `create_settled`, `settle`) is the one
//! `environment_lifecycle` already owns, so every test drives the service the way a
//! client does: `POST /` with `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::entitlement_connector::connector_in;
use crate::environment_lifecycle::{call, create_settled, emulator, error_type, settle, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] = evs::ops::types::CONNECTOR_TYPE_VALUES;

/// The type of connector an entitlement is created through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// Create `vm_ids` entitlements through one vCenter connector.
async fn create_entitlements(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    connector_id: &str,
    vm_ids: &[String],
) {
    if vm_ids.is_empty() {
        return;
    }
    let (status, body) = call(
        router,
        "CreateEntitlement",
        &json!({
            "environmentId": environment_id,
            "connectorId": connector_id,
            "entitlementType": WINDOWS_SERVER,
            "vmIds": vm_ids,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating {} entitlement(s) expected 200, got {status} {body}",
        vm_ids.len()
    );
}

/// Every connector record one environment holds, as `ListEnvironmentConnectors` reports
/// them. Each environment here holds fewer than one page, so a `nextToken` would be a
/// defect of the list rather than a page to follow.
async fn listed_connectors(router: &Emulator, case: &str, environment_id: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "ListEnvironmentConnectors",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the connector list expected 200, got {status} {body}"
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

/// The environment record `GetEnvironment` reports.
async fn fetched_environment(router: &Emulator, case: &str, environment_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GetEnvironment",
        &json!({ "environmentId": environment_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnvironment expected 200, got {status} {body}"
    );
    body["environment"].clone()
}

/// Every entitlement record one environment holds, read from the store.
///
/// The store rather than the wire, because an entitlement is listed THROUGH the
/// connector it was created through and that connector goes away with the environment:
/// the refusal the list then answers is asserted separately, and this is what shows what
/// became of the record behind it.
fn stored_entitlements(router: &Emulator, environment_id: &str) -> Vec<evs::state::StoredEntitlement> {
    let store = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = store.lock();
    store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("the store holds the environment {environment_id}"))
        .entitlements
        .values()
        .cloned()
        .collect()
}

/// Assert that every connector of one environment is in `expected_state` and carries
/// `expected_modified_at` — the environment's own `modifiedAt`, because an environment
/// and the connectors its deletion takes with it move on one clock.
async fn assert_connectors(
    router: &Emulator,
    case: &str,
    environment_id: &str,
    expected_count: usize,
    expected_state: &str,
    expected_modified_at: &Value,
) {
    let listed = listed_connectors(router, case, environment_id).await;
    assert_eq!(
        listed.len(),
        expected_count,
        "case {case:?}: expected {expected_count} connector(s), got {listed:?}"
    );
    for connector in &listed {
        assert_eq!(
            connector["state"],
            json!(expected_state),
            "case {case:?}: connector {}: expected state {expected_state}, got {}",
            connector["connectorId"],
            connector["state"]
        );
        assert_eq!(
            &connector["modifiedAt"], expected_modified_at,
            "case {case:?}: connector {}: expected modifiedAt {expected_modified_at}, got {}",
            connector["connectorId"], connector["modifiedAt"]
        );
    }
}

#[tokio::test]
async fn deleting_an_environment_deletes_its_connectors_and_stops_its_entitlements() {
    let router = emulator();

    // A control environment with its own connector and entitlement, deleted by nothing:
    // the cascade is scoped to the environment the delete named, so this one keeps a
    // live connector and a tracking entitlement throughout.
    let control = create_settled(&router, None).await;
    let control_connector = connector_in(&router, "the control environment", &control, VCENTER).await;
    create_entitlements(
        &router,
        "the control environment",
        &control,
        &control_connector,
        &["vm-1".to_string()],
    )
    .await;

    // Each row: the case, the connector types to create in the environment, the VMs to
    // entitle through its vCenter connector, and whether the records' own CREATING
    // changes have come due before the delete. The rows differ along what the rule
    // generalises over — the number and the TYPE of the sub-resources (the whole
    // modelled connector catalog), how many entitlements there are (none, one, several,
    // and a batch), and the state each record is deleted FROM (`CREATING` as well as
    // the settled `ACTIVE` / `CREATED`).
    let rows: Vec<(&str, Vec<&str>, Vec<String>, bool)> = vec![
        (
            "one ACTIVE vCenter connector tracking one VM",
            vec![VCENTER],
            vec!["vm-1".to_string()],
            true,
        ),
        (
            "a CREATING vCenter connector whose entitlements are still being validated",
            vec![VCENTER],
            vec!["vm-1".to_string(), "vm-22".to_string(), "vm-333".to_string()],
            false,
        ),
        (
            "every modelled connector type at once, and no entitlement",
            CONNECTOR_TYPES.to_vec(),
            Vec::new(),
            true,
        ),
        (
            "a vCenter connector beside another type, tracking a batch of VMs",
            vec![VCENTER, "SDDC_MANAGER"],
            (1..=10).map(|ordinal| format!("vm-{ordinal}")).collect(),
            true,
        ),
    ];
    // Every modelled type is covered by the rows above, so the catalog and the table
    // cannot drift apart.
    for connector_type in CONNECTOR_TYPES {
        assert!(
            rows.iter()
                .any(|(_, types, _, _)| types.contains(connector_type)),
            "the table must cover the modelled type {connector_type:?}"
        );
    }

    for (case, connector_types, vm_ids, settle_first) in rows {
        let environment = create_settled(&router, None).await;
        let mut vcenter: Option<String> = None;
        for connector_type in &connector_types {
            let connector_id = connector_in(&router, case, &environment, connector_type).await;
            if *connector_type == VCENTER {
                vcenter = Some(connector_id);
            }
        }
        let vcenter = vcenter.unwrap_or_else(|| panic!("case {case:?}: every row holds a vCenter"));
        create_entitlements(&router, case, &environment, &vcenter, &vm_ids).await;
        if settle_first {
            settle(&router);
        }

        // Before the delete every record of the environment is LIVE: that is what the
        // delete has to change.
        let before = listed_connectors(&router, case, &environment).await;
        for connector in &before {
            let state = connector["state"].as_str().unwrap_or_default();
            assert_eq!(
                state,
                if settle_first { "ACTIVE" } else { "CREATING" },
                "case {case:?}: before the delete the connector is live, got {connector}"
            );
        }
        for entitlement in stored_entitlements(&router, &environment) {
            assert_eq!(
                entitlement.status,
                if settle_first { "CREATED" } else { "CREATING" },
                "case {case:?}: before the delete the entitlement of {} tracks usage, got {:?}",
                entitlement.vm_id,
                entitlement.status
            );
            assert!(
                entitlement.stopped_at_ms.is_none(),
                "case {case:?}: before the delete nothing has stopped the entitlement of {}, \
                 got {:?}",
                entitlement.vm_id,
                entitlement.stopped_at_ms
            );
        }

        let (status, deleted) = call(
            &router,
            "DeleteEnvironment",
            &json!({ "environmentId": environment }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete expected 200, got {status} {deleted}"
        );
        let deleting = deleted["environment"].clone();
        assert_eq!(
            deleting["environmentState"],
            json!("DELETING"),
            "case {case:?}: the delete reports the environment going away, got {deleting}"
        );
        let deleting_at = deleting["modifiedAt"].clone();
        let deleting_at_ms = epoch_millis(&deleting_at);

        // The connectors are going away WITH the environment, on its clock.
        assert_connectors(
            &router,
            case,
            &environment,
            connector_types.len(),
            "DELETING",
            &deleting_at,
        )
        .await;

        // And the entitlements have stopped tracking usage at that same instant: the
        // record is kept so a later read still reports the stop, with the `startedAt` it
        // had and the `stoppedAt` the deletion stamped.
        let stopped = stored_entitlements(&router, &environment);
        assert_eq!(
            stopped.len(),
            vm_ids.len(),
            "case {case:?}: expected {} entitlement record(s), got {stopped:?}",
            vm_ids.len()
        );
        for entitlement in &stopped {
            assert_eq!(
                entitlement.status, "DELETED",
                "case {case:?}: the entitlement of {} must stop tracking usage, got {:?}",
                entitlement.vm_id, entitlement.status
            );
            assert_eq!(
                entitlement.stopped_at_ms,
                Some(deleting_at_ms),
                "case {case:?}: the entitlement of {} must stop at the instant the deletion \
                 was accepted ({deleting_at_ms}), got {:?}",
                entitlement.vm_id,
                entitlement.stopped_at_ms
            );
            assert_eq!(
                entitlement.started_at_ms.is_some(),
                settle_first,
                "case {case:?}: the entitlement of {} keeps the startedAt it had, got {:?}",
                entitlement.vm_id,
                entitlement.started_at_ms
            );
            assert!(
                entitlement.pending.is_none(),
                "case {case:?}: a stopped entitlement owes no further change, got {:?}",
                entitlement.vm_id
            );
        }

        // Over the wire the entitlements are no longer listable at all: the connector
        // they are read through is being torn down with the environment.
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &json!({
                "environmentId": environment,
                "connectorId": vcenter,
                "entitlementType": WINDOWS_SERVER,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a list through a connector going away with the environment is \
             refused, got {status} {body}"
        );
        assert_eq!(
            error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["resourceId"],
            json!(vcenter),
            "case {case:?}: resourceId must echo the requested connector, got {body}"
        );

        // Once the deletion comes due the environment and its connectors are DELETED
        // together: one clock, one `modifiedAt`.
        settle(&router);
        let settled = fetched_environment(&router, case, &environment).await;
        assert_eq!(
            settled["environmentState"],
            json!("DELETED"),
            "case {case:?}: the environment settles to DELETED, got {settled}"
        );
        assert_connectors(
            &router,
            case,
            &environment,
            connector_types.len(),
            "DELETED",
            &settled["modifiedAt"],
        )
        .await;

        // The control environment is untouched by every delete above.
        let control_connectors = listed_connectors(&router, case, &control).await;
        assert_eq!(
            control_connectors.len(),
            1,
            "case {case:?}: the control environment keeps its connector, got {control_connectors:?}"
        );
        assert_eq!(
            control_connectors[0]["state"],
            json!("ACTIVE"),
            "case {case:?}: the control environment's connector stays live, got {}",
            control_connectors[0]
        );
        let (status, body) = call(
            &router,
            "ListVmEntitlements",
            &json!({
                "environmentId": control,
                "connectorId": control_connector,
                "entitlementType": WINDOWS_SERVER,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control environment's entitlements stay listable, got \
             {status} {body}"
        );
        let control_entitlements = body["entitlements"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no entitlements list in {body}"))
            .clone();
        assert_eq!(
            control_entitlements.len(),
            1,
            "case {case:?}: the control environment keeps its entitlement, got \
             {control_entitlements:?}"
        );
        assert_eq!(
            control_entitlements[0]["status"],
            json!("CREATED"),
            "case {case:?}: the control environment's entitlement keeps tracking usage, got {}",
            control_entitlements[0]
        );
        assert!(
            control_entitlements[0]["stoppedAt"].is_null(),
            "case {case:?}: nothing has stopped the control environment's entitlement, got {}",
            control_entitlements[0]
        );
    }
}

/// The epoch milliseconds a reported timestamp carries — awsJson1_0 renders a
/// `Timestamp` as epoch SECONDS, so this is the stored stamp behind it.
fn epoch_millis(reported: &Value) -> i64 {
    let seconds = reported
        .as_f64()
        .unwrap_or_else(|| panic!("the reported timestamp {reported} is a number"));
    (seconds * 1000.0).round() as i64
}

#[tokio::test]
async fn a_deleted_environment_holds_no_resource_created_while_it_was_going_away() {
    // The terminal half of the same rule: whatever reached the environment while its
    // deletion was in flight is gone once the deletion comes due, so a DELETED
    // environment holds no live connector and no tracking entitlement at all.
    let router = emulator();
    let environment = create_settled(&router, None).await;
    let first = connector_in(&router, "before the delete", &environment, VCENTER).await;
    create_entitlements(
        &router,
        "before the delete",
        &environment,
        &first,
        &["vm-1".to_string()],
    )
    .await;

    let (status, body) = call(
        &router,
        "DeleteEnvironment",
        &json!({ "environmentId": environment }),
    )
    .await;
    assert_eq!(status, 200, "the delete expected 200, got {status} {body}");

    // A connector created while the deletion is in flight — of a type the environment
    // holds no record of, the one kind of sub-resource a deletion in flight leaves room
    // for: an entitlement needs a LIVE vCenter connector to be created through, and this
    // environment's went away with the delete above.
    let during = connector_in(&router, "during the delete", &environment, "SDDC_MANAGER").await;

    settle(&router);
    let settled = fetched_environment(&router, "during the delete", &environment).await;
    assert_eq!(
        settled["environmentState"],
        json!("DELETED"),
        "the environment settles to DELETED, got {settled}"
    );
    let listed = listed_connectors(&router, "during the delete", &environment).await;
    for connector in &listed {
        assert_eq!(
            connector["state"],
            json!("DELETED"),
            "connector {}: a DELETED environment holds no live connector, got {connector}",
            connector["connectorId"]
        );
    }
    let ids: Vec<&str> = listed
        .iter()
        .filter_map(|connector| connector["connectorId"].as_str())
        .collect();
    for expected in [&first, &during] {
        assert!(
            ids.contains(&expected.as_str()),
            "the list must still report the connector {expected}, got {ids:?}"
        );
    }
    for entitlement in stored_entitlements(&router, &environment) {
        assert_eq!(
            entitlement.status, "DELETED",
            "the entitlement of {} tracks usage no longer, got {:?}",
            entitlement.vm_id, entitlement.status
        );
        assert!(
            entitlement.stopped_at_ms.is_some(),
            "the entitlement of {} carries the instant tracking stopped, got {:?}",
            entitlement.vm_id,
            entitlement.stopped_at_ms
        );
    }
}
