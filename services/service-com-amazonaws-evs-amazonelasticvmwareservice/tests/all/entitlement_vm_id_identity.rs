// SPDX-License-Identifier: CC0-1.0

//! The one rule both entitlement handlers fold a `vmIds` list through, end to end over
//! awsJson1_0.
//!
//! The (connector, VM) pair is what identifies an entitlement —
//! `crate::state::entitlement_key`, the key `CreateEntitlement`, `ListVmEntitlements`
//! and `DeleteEntitlement` all address entitlements by — so a `vmIds` list naming the
//! same virtual machine twice names ONE entitlement, and each of the two operations
//! that take a `vmIds` list reports it once, in the order the request first named it.
//!
//! `CreateEntitlement` and `DeleteEntitlement` reach that rule through the single
//! `crate::state::distinct_vm_ids` it is stated in, so this suite drives ONE table of
//! `vmIds` lists through BOTH operations: the create that mints the records and the
//! delete that removes them must name exactly the same virtual machines, in the same
//! order, for every row.
//!
//! The harness (`emulator`, `call`) is the one `environment_lifecycle` already owns and
//! the environment-with-a-vCenter-connector setup is `entitlement_connector`'s, so every
//! test drives the service the way a client does: `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>`.

use serde_json::{json, Value};

use crate::entitlement_connector::environment_with_vcenter_connector;
use crate::environment_lifecycle::{call, emulator, Emulator};

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// A `CreateEntitlement`/`DeleteEntitlement` request body for `vm_ids`, through
/// `connector_id`: the two operations take the same four members.
fn entitlement_request(environment_id: &str, connector_id: &str, vm_ids: &[String]) -> Value {
    json!({
        "environmentId": environment_id,
        "connectorId": connector_id,
        "entitlementType": WINDOWS_SERVER,
        "vmIds": vm_ids,
    })
}

/// The `vmId`s of a response's `entitlements`, in the order the response reported them.
fn reported_vm_ids(case: &str, operation: &str, body: &Value) -> Vec<String> {
    body["entitlements"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {operation} reported no entitlements list in {body}"))
        .iter()
        .map(|entry| {
            entry["vmId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {operation}: no vmId in {entry}"))
                .to_string()
        })
        .collect()
}

/// Call `operation` with `vm_ids` and return the `vmId`s it reported, in order.
async fn reported(
    router: &Emulator,
    case: &str,
    operation: &str,
    environment_id: &str,
    connector_id: &str,
    vm_ids: &[String],
) -> Vec<String> {
    let (status, body) = call(
        router,
        operation,
        &entitlement_request(environment_id, connector_id, vm_ids),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} {body}"
    );
    reported_vm_ids(case, operation, &body)
}

/// `vm_ids` as the owned strings a request body carries.
fn vms(vm_ids: &[&str]) -> Vec<String> {
    vm_ids.iter().map(|vm| (*vm).to_string()).collect()
}

#[tokio::test]
async fn one_vm_ids_list_names_one_entitlement_per_distinct_vm_in_both_operations() {
    let router = emulator();

    // The longest `vmId` the model admits (`VmId`, len 4..1024, pattern ^vm-[0-9]+$).
    let longest = format!("vm-{}", "1".repeat(1021));
    // The modelled maximum of 100 `vmIds` (`VmIdList`, len 1..100), every entry the same
    // virtual machine.
    let hundred_of_one = vec!["vm-9".to_string(); 100];
    // The same bound, with 50 distinct machines each named twice, interleaved.
    let fifty_pairs: Vec<String> = (1..=50)
        .flat_map(|ordinal| [format!("vm-{ordinal}"), format!("vm-{ordinal}")])
        .collect();
    let fifty_distinct: Vec<String> = (1..=50).map(|ordinal| format!("vm-{ordinal}")).collect();

    // Each row: the case, the `vmIds` the request carries, and the distinct machines it
    // names in first-seen order. The rows differ along what the rule generalises over —
    // how many entries repeat, how far apart the repeats are, how many times one machine
    // is named, the length of a repeated id, ids that only look alike, and the modelled
    // maximum number of entries.
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "no repeat at all",
            vms(&["vm-1", "vm-22", "vm-333"]),
            vms(&["vm-1", "vm-22", "vm-333"]),
        ),
        (
            "adjacent repeat",
            vms(&["vm-7", "vm-7", "vm-8"]),
            vms(&["vm-7", "vm-8"]),
        ),
        (
            "repeat several entries apart",
            vms(&["vm-1", "vm-2", "vm-3", "vm-1"]),
            vms(&["vm-1", "vm-2", "vm-3"]),
        ),
        (
            "a repeat before a machine first seen later",
            vms(&["vm-4", "vm-4", "vm-5", "vm-4", "vm-6"]),
            vms(&["vm-4", "vm-5", "vm-6"]),
        ),
        (
            "one machine named five times",
            vms(&["vm-5", "vm-5", "vm-5", "vm-5", "vm-5"]),
            vms(&["vm-5"]),
        ),
        (
            "ids that only look alike are distinct machines",
            vms(&["vm-01", "vm-1", "vm-01", "vm-10"]),
            vms(&["vm-01", "vm-1", "vm-10"]),
        ),
        (
            "a vmId at the modelled 1024-character maximum, named twice",
            vec![longest.clone(), "vm-1".to_string(), longest.clone()],
            vec![longest.clone(), "vm-1".to_string()],
        ),
        (
            "the modelled maximum of 100 entries, all one machine",
            hundred_of_one.clone(),
            vms(&["vm-9"]),
        ),
        (
            "the modelled maximum of 100 entries, 50 machines named twice",
            fifty_pairs.clone(),
            fifty_distinct.clone(),
        ),
    ];

    for (case, requested, distinct) in rows {
        // A fresh environment with its own vCenter connector per row, so each row
        // creates and deletes only its own records.
        let (environment, connector) = environment_with_vcenter_connector(&router, case).await;

        // The create: one record per distinct machine, reported in first-seen order.
        let created = reported(
            &router,
            case,
            "CreateEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert_eq!(
            created, distinct,
            "case {case:?}: CreateEntitlement must report one entitlement per distinct \
             virtual machine, expected {distinct:?}, got {created:?}"
        );

        // The delete: the SAME machines the create named, from the same list, so the two
        // operations cannot disagree about which machines one request names.
        let deleted = reported(
            &router,
            case,
            "DeleteEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert_eq!(
            deleted, distinct,
            "case {case:?}: DeleteEntitlement must report one entitlement per distinct \
             virtual machine, expected {distinct:?}, got {deleted:?}"
        );
        assert_eq!(
            deleted, created,
            "case {case:?}: both operations fold one vmIds list the same way, expected \
             {created:?}, got {deleted:?}"
        );

        // And the store is empty afterwards: the delete removed exactly the records the
        // create minted, so nothing is left for the same list to name.
        let left = reported(
            &router,
            case,
            "DeleteEntitlement",
            &environment,
            &connector,
            &requested,
        )
        .await;
        assert!(
            left.is_empty(),
            "case {case:?}: every record the create minted was removed, so a repeat \
             delete removes nothing, got {left:?}"
        );
    }
}
