// SPDX-License-Identifier: CC0-1.0

//! `RebootInstance` — restarting a Lightsail instance in place, driven over the
//! wire through the same front door a client uses.
//!
//! The rule under test: a reboot restarts the instance's operating system WITHOUT
//! re-provisioning the instance, so every member of the one stored record — every
//! address the store assigned it above all — reads back identical afterwards and
//! the instance keeps serving; the call's only write is the `RebootInstance` entry
//! it adds to the operation log, which is answered non-terminal and is readable
//! back by its id through `GetOperation` until the clock settles it. That is what
//! separates a reboot from a stop and start, after which Lightsail assigns a new
//! public IP address (the modelled `StopInstance` documentation).
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! the instance's IP address type (so the addresses it has at all differ), its
//! blueprint platform, its bundle size and its Region — the recorded case is one
//! row, not the case. An instance with no running operating system to restart
//! (stopped, or on its way down) is refused with the modelled
//! `OperationFailureException`, and a name that was never created is refused with
//! the service's `DoesNotExist` envelope.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// One instance to reboot. The rows differ in IP address type (which decides
/// which addresses the instance has), blueprint platform, bundle size and Region.
#[derive(Debug)]
struct Row {
    case: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// `None` leaves `ipAddressType` out of the request, which the service
    /// defaults to `dualstack`.
    ip_address_type: Option<&'static str>,
}

/// The instances every case here reboots.
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux, no ipAddressType (dualstack by default)",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
    },
    Row {
        case: "ubuntu, ipv4 only, smallest bundle",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
    },
    Row {
        case: "windows, ipv6 only, so there is no public IPv4 address at all",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("ipv6"),
    },
    Row {
        case: "application blueprint, dualstack, largest bundle in the table",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("dualstack"),
    },
];

/// Names nothing created, differing along the length, the case, the digits and the
/// separators. Every one satisfies the modelled `ResourceName` pattern
/// `^\w[\w\-]*\w$`, so the centralized walk passes it through to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// The `CreateInstances` body for `row`, naming one instance.
fn create_body(row: &Row, name: &str) -> Value {
    let mut body = json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    });
    if let Some(ip_address_type) = row.ip_address_type {
        body["ipAddressType"] = json!(ip_address_type);
    }
    body
}

/// Poll `GetInstanceState` until the instance `name` reports `running`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until_running(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = emulator
            .ok("GetInstanceState", json!({"instanceName": name}))
            .await;
        if body["state"]["name"].as_str() == Some("running") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {name} to run: the clock never settled it, last {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Assert `operations` is exactly one `RebootInstance` log entry against `name`,
/// answering its id.
fn assert_reboot_operation(case: &str, operations: &Value, name: &str, region: &str) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: a reboot acts on one instance, so it logs one operation, got {operations:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("RebootInstance"),
        "case {case}: expected operationType RebootInstance, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(name),
        "case {case}: expected resourceName {name}, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Instance"),
        "case {case}: expected resourceType Instance, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where the instance is, got {operation:?}"
    );
    // A restart is not finished when the call returns, so the entry is answered
    // non-terminal and the clock owes it its `Succeeded` status.
    assert_eq!(
        (
            operation["status"].as_str(),
            operation["isTerminal"].as_bool()
        ),
        (Some("Started"), Some(false)),
        "case {case}: expected a non-terminal Started entry, got {operation:?}"
    );
    let id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a minted operation id, got {operation:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

// ---------------------------------------------------------------------------
// The reboot itself
// ---------------------------------------------------------------------------

/// RULE: a reboot restarts the instance in place — every member of the one stored
/// record, and every address the store assigned it, reads back identical
/// afterwards, and the instance keeps serving — while the call logs one
/// non-terminal `RebootInstance` operation against that instance.
#[tokio::test]
async fn rebooting_an_instance_leaves_every_stored_member_untouched() {
    for row in ROWS {
        let emulator = Emulator::new();
        let name = "rebooted";
        emulator
            .ok("CreateInstances", create_body(row, name))
            .await;
        wait_until_running(&emulator, name).await;

        let before = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();
        // The addresses the rule is about must actually be there to be preserved:
        // an IPv4 instance has a public IPv4 address, an IPv6-only one has IPv6
        // addresses and no public IPv4 address at all.
        let wants_ipv4 = row.ip_address_type.is_none_or(|kind| kind != "ipv6");
        assert_eq!(
            before["publicIpAddress"].is_string(),
            wants_ipv4,
            "case {}: an instance with ipAddressType {:?} must report a public IPv4 \
             address only when it has one, got {before:?}",
            row.case,
            row.ip_address_type
        );
        assert!(
            !before["ipv6Addresses"]
                .as_array()
                .unwrap_or(&Vec::new())
                .is_empty()
                || wants_ipv4,
            "case {}: an IPv6 instance must report IPv6 addresses, got {before:?}",
            row.case
        );

        let rebooted = emulator
            .ok("RebootInstance", json!({"instanceName": name}))
            .await;
        let first_id =
            assert_reboot_operation(row.case, &rebooted["operations"], name, row.region);

        let after = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();
        // The whole record, member by member: a reboot re-provisions nothing.
        assert_eq!(
            after, before,
            "case {}: the reboot must leave the stored instance identical, got {after:?}",
            row.case
        );
        // Spelled out for the members the rule is about, so a failure says which.
        for member in [
            "publicIpAddress",
            "privateIpAddress",
            "ipv6Addresses",
            "arn",
            "supportCode",
            "createdAt",
            "sshKeyName",
            "bundleId",
            "blueprintId",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {}: the reboot must preserve {member}, expected {:?}, got {:?}",
                row.case, before[member], after[member]
            );
        }
        // A rebooted instance keeps serving: the restart is inside the instance,
        // so its lifecycle state does not move.
        assert_eq!(
            (
                after["state"]["name"].as_str(),
                after["state"]["code"].as_i64()
            ),
            (Some("running"), Some(16)),
            "case {}: a rebooted instance stays running, got {:?}",
            row.case,
            after["state"]
        );

        // The entry the reboot logged is readable by its id, and it is the SAME
        // record the reboot answered with: one log, not two. Only its status moves
        // (the clock owes it `Succeeded`), so the identity members are what must
        // match.
        let read_back = emulator
            .ok("GetOperation", json!({"operationId": first_id}))
            .await;
        for member in [
            "id",
            "operationType",
            "resourceName",
            "resourceType",
            "operationDetails",
            "createdAt",
            "location",
        ] {
            assert_eq!(
                read_back["operation"][member], rebooted["operations"][0][member],
                "case {}: GetOperation must report the entry the reboot logged, \
                 expected {member} {:?}, got {:?}",
                row.case, rebooted["operations"][0][member], read_back["operation"][member]
            );
        }

        // Rebooting again is accepted the same way, logs a DISTINCT entry, and
        // still changes nothing on the record.
        let again = emulator
            .ok("RebootInstance", json!({"instanceName": name}))
            .await;
        let second_id = assert_reboot_operation(
            &format!("{}, rebooted twice", row.case),
            &again["operations"],
            name,
            row.region,
        );
        assert_ne!(
            first_id, second_id,
            "case {}: each reboot logs its own operation, got {first_id} twice",
            row.case
        );
        let twice = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();
        assert_eq!(
            twice, before,
            "case {}: a second reboot must leave the record identical too, got {twice:?}",
            row.case
        );
    }
}

/// RULE: an instance that is still coming up is accepted — it is already on its
/// way to serving — and the reboot leaves the transition the clock already owes it
/// in place, so it still settles into `running` without anyone starting it again.
#[tokio::test]
async fn rebooting_an_instance_that_is_coming_up_keeps_the_transition_it_is_owed() {
    let emulator = Emulator::new();
    let name = "still-coming-up";
    emulator
        .ok("CreateInstances", create_body(&ROWS[0], name))
        .await;

    // No wait: the instance is `pending` for as long as the clock owes it
    // `running`.
    let rebooted = emulator
        .ok("RebootInstance", json!({"instanceName": name}))
        .await;
    assert_reboot_operation(
        "rebooting an instance that is coming up",
        &rebooted["operations"],
        name,
        ROWS[0].region,
    );

    wait_until_running(&emulator, name).await;
    let instance = emulator.ok("GetInstance", json!({"instanceName": name})).await
        ["instance"]
        .clone();
    assert!(
        instance["publicIpAddress"].is_string(),
        "case rebooting an instance that is coming up: it still has the address the \
         create assigned it, got {instance:?}"
    );
}

// ---------------------------------------------------------------------------
// The operation log
// ---------------------------------------------------------------------------

/// RULE: a reboot's log entry is answered non-terminal, and the clock owes it its
/// terminal `Succeeded` status — which the next read of that entry reports,
/// without anyone polling for it more than once. An id the log does not hold is
/// refused with the service's `DoesNotExist` envelope.
#[tokio::test]
async fn the_clock_settles_the_reboot_it_logged() {
    let emulator = Emulator::new();
    let name = "settling";
    emulator
        .ok("CreateInstances", create_body(&ROWS[2], name))
        .await;
    wait_until_running(&emulator, name).await;

    let rebooted = emulator
        .ok("RebootInstance", json!({"instanceName": name}))
        .await;
    let id = assert_reboot_operation(
        "the entry a reboot logs",
        &rebooted["operations"],
        name,
        ROWS[2].region,
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let settled = loop {
        let body = emulator.ok("GetOperation", json!({"operationId": id})).await;
        if body["operation"]["status"].as_str() == Some("Succeeded") {
            break body["operation"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case the entry a reboot logs: the clock never settled it, last {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(
        settled["isTerminal"].as_bool(),
        Some(true),
        "case the entry a reboot logs: a settled entry is terminal, got {settled:?}"
    );
    assert_eq!(
        settled["id"].as_str(),
        Some(id.as_str()),
        "case the entry a reboot logs: the id must not change as it settles, got {settled:?}"
    );

    // Ids the log does not hold, differing in form: a well-formed GUID nothing
    // minted, a shape that is not a GUID at all, and the shortest the modelled
    // `NonEmptyString` pattern `\S` admits.
    for unknown in [
        "00000000-0000-0000-0000-000000000000",
        "not-a-guid",
        "x",
    ] {
        let (status, body) = emulator
            .call("GetOperation", json!({"operationId": unknown}))
            .await;
        assert_eq!(
            (status, error_code(&body), body["code"].as_str()),
            (400, "NotFoundException".to_string(), Some("DoesNotExist")),
            "case GetOperation on {unknown:?}: expected the DoesNotExist envelope, \
             got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: there has to be a running operating system to restart, so an instance
/// that is stopped — or on its way down — is refused with the modelled
/// `OperationFailureException` (HTTP 400) naming the instance and the state it is
/// in, and the record is left exactly as it was.
#[tokio::test]
async fn rebooting_an_instance_with_nothing_to_restart_is_refused() {
    // Both ways an instance ends up with no running OS: a forced stop, which
    // completes as the call returns, and an ordinary stop, which passes through
    // `stopping` first.
    for (case, force) in [("a forced stop", true), ("an ordinary stop", false)] {
        let emulator = Emulator::new();
        let name = "down";
        emulator
            .ok("CreateInstances", create_body(&ROWS[1], name))
            .await;
        wait_until_running(&emulator, name).await;

        let mut stop = json!({"instanceName": name});
        if force {
            stop["force"] = json!(true);
        }
        emulator.ok("StopInstance", stop).await;
        let before = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();

        let (status, body) = emulator
            .call("RebootInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected the modelled OperationFailureException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(name),
            "case {case}: the refusal must name the instance, got {body:?}"
        );
        // Whether the stop has already settled decides which of the two states the
        // message reports; both are states with nothing to restart.
        assert!(
            message.contains("stopped") || message.contains("stopping"),
            "case {case}: the refusal must name the state that is in the way, got {body:?}"
        );

        // A refused reboot changes nothing: the record — addresses included — is
        // what it was, and no reboot is reported as having happened.
        let after = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();
        assert_eq!(
            after["publicIpAddress"], before["publicIpAddress"],
            "case {case}: a refused reboot must not touch the addresses, got {after:?}"
        );
        assert_eq!(
            after["arn"], before["arn"],
            "case {case}: a refused reboot must not touch the record, got {after:?}"
        );
    }
}

/// RULE: a reboot resolves `instanceName` against the store, so a name that was
/// never created — or has since been deleted — is refused with the service's
/// `DoesNotExist` envelope and nothing is stored.
#[tokio::test]
async fn rebooting_an_unknown_instance_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call("RebootInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {name:?}: expected the wire error NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {name:?}: expected the body code DoesNotExist, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Instance does not exist: {name}").as_str()),
            "case {name:?}: expected the message to name the resource type and the \
             requested name, got {body:?}"
        );
    }

    // A deleted instance is refused exactly like one that never existed.
    let name = "short-lived";
    emulator
        .ok("CreateInstances", create_body(&ROWS[0], name))
        .await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;
    let (status, body) = emulator
        .call("RebootInstance", json!({"instanceName": name}))
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("DoesNotExist")),
        "case after the delete: expected the DoesNotExist envelope, got {status} {body:?}"
    );

    // Refusing stored nothing: the store holds no instance at all.
    let listed = emulator.ok("GetInstances", json!({})).await;
    assert_eq!(
        listed["instances"].as_array().map(Vec::len),
        Some(0),
        "case after every refused reboot: the store must be empty, got {listed:?}"
    );
}
