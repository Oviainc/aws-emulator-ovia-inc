// SPDX-License-Identifier: CC0-1.0

//! `DeleteKnownHostKeys` — deleting the host keys Lightsail has recorded for an
//! instance, driven over the wire through the same front door a client uses.
//!
//! The rule under test: the call resolves `instanceName` against the ONE stored
//! instance record, empties the host keys the service holds as known for THAT
//! instance — whatever was recorded for it, and nothing recorded for any other
//! instance — and appends exactly one `DeleteKnownHostKeys` entry to the
//! operation log, located where the instance is and answered TERMINAL, because
//! the recorded keys are gone when the call returns and nothing about them is
//! left for the clock to settle. Nothing else on the instance record moves, the
//! call is idempotent (an instance with no recorded keys is served the same way),
//! and a name the store does not hold is refused with the service's
//! `DoesNotExist` envelope without logging anything.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//! how many host keys are recorded for the instance (none, one, several), the
//! FORM of each record (an SSH host key with an algorithm and a public key, or an
//! RDP certificate with its fingerprints and validity window), and the instance
//! the record hangs off — its blueprint platform, bundle size, Region and IP
//! address type, none of which the answer depends on, which is what the rows
//! prove.
//!
//! `GetInstanceAccessDetails` — the browser-based access path that RECORDS a host
//! key, "the time that the SSH host key or RDP certificate was recorded by
//! Lightsail" (`HostKeyAttributes$witnessedAt`) — is still a 501 stub with its own
//! issue, so the record this operation deletes cannot be made through the API yet:
//! it is made here on the instance's own record through the same public state API
//! a handler uses, so the delete is exercised against the one persisted record
//! rather than a copy.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::HostKeyAttributes;
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The `operationType` every entry this suite is about carries, the modelled
/// `OperationType` enum's own `DeleteKnownHostKeys` member.
const OPERATION_TYPE: &str = "DeleteKnownHostKeys";

/// An instant to record a host key at, far enough in the past to be plainly a
/// recorded value rather than the instant of a call: 2026-10-24T01:05:05Z.
const WITNESSED_AT_MS: i64 = 1_792_890_305_000;

/// One instance whose recorded host keys a case deletes, and what is recorded
/// for it. The rows differ in how many records there are, the form of each, and
/// the instance they hang off.
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
    /// The host keys the service has recorded for the instance before the call.
    recorded: &'static [Recorded],
}

/// One recorded host key, in either of the two forms the modelled
/// `HostKeyAttributes` takes: an SSH host key, which carries the key's algorithm
/// and the public key itself, or an RDP certificate, which carries its
/// fingerprints and the window it is valid in.
#[derive(Debug)]
enum Recorded {
    /// `algorithm` ("The SSH host key algorithm or the RDP certificate
    /// fingerprint algorithm") and `publicKey` ("The public SSH host key or the
    /// RSA fingerprint of the certificate").
    SshHostKey {
        algorithm: &'static str,
        public_key: &'static str,
    },
    /// A certificate record: `fingerprintSHA1` / `fingerprintSHA256` and the
    /// `notValidBefore` / `notValidAfter` window.
    RdpCertificate {
        fingerprint_sha1: &'static str,
        fingerprint_sha256: &'static str,
        valid_from_ms: i64,
        valid_to_ms: i64,
    },
}

impl Recorded {
    /// This record as the modelled shape the service stores.
    fn attributes(&self, witnessed_at_ms: i64) -> HostKeyAttributes {
        match self {
            Self::SshHostKey {
                algorithm,
                public_key,
            } => HostKeyAttributes {
                algorithm: Some((*algorithm).to_string()),
                public_key: Some((*public_key).to_string()),
                witnessed_at: Some(lightsail_state::timestamp(witnessed_at_ms)),
                ..Default::default()
            },
            Self::RdpCertificate {
                fingerprint_sha1,
                fingerprint_sha256,
                valid_from_ms,
                valid_to_ms,
            } => HostKeyAttributes {
                fingerprint_sha1: Some((*fingerprint_sha1).to_string()),
                fingerprint_sha256: Some((*fingerprint_sha256).to_string()),
                witnessed_at: Some(lightsail_state::timestamp(witnessed_at_ms)),
                not_valid_before: Some(lightsail_state::timestamp(*valid_from_ms)),
                not_valid_after: Some(lightsail_state::timestamp(*valid_to_ms)),
                ..Default::default()
            },
        }
    }
}

/// The table every case here runs. The blueprint ids, bundle ids, Regions and
/// availability zones are the ones this crate's other instance suites use, so a
/// row stands up through `CreateInstances` unchanged.
const ROWS: &[Row] = &[
    Row {
        case: "amazon linux, one recorded ssh host key",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        ip_address_type: None,
        recorded: &[Recorded::SshHostKey {
            algorithm: "ssh-ed25519",
            public_key: "AAAAC3NzaC1lZDI1NTE5AAAAIEmulatorRecordedHostKeyOne",
        }],
    },
    Row {
        case: "ubuntu, ipv4 only, three recorded ssh host keys of different algorithms",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        ip_address_type: Some("ipv4"),
        recorded: &[
            Recorded::SshHostKey {
                algorithm: "ssh-rsa",
                public_key: "AAAAB3NzaC1yc2EAAAADAQABAAABgEmulatorRecordedHostKeyTwo",
            },
            Recorded::SshHostKey {
                algorithm: "ecdsa-sha2-nistp256",
                public_key: "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAEmulatorRecordedHostKeyThree",
            },
            Recorded::SshHostKey {
                algorithm: "ssh-ed25519",
                public_key: "AAAAC3NzaC1lZDI1NTE5AAAAIEmulatorRecordedHostKeyFour",
            },
        ],
    },
    Row {
        case: "windows, ipv6 only, a recorded RDP certificate instead of an ssh host key",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "windows_server_2022",
        bundle_id: "large_3_0",
        ip_address_type: Some("ipv6"),
        recorded: &[Recorded::RdpCertificate {
            fingerprint_sha1: "SHA1:9a1b2c3d4e5f60718293a4b5c6d7e8f901234567",
            fingerprint_sha256: "SHA256:0f1e2d3c4b5a69788796a5b4c3d2e1f00112233445566778899aabbccddeeff0",
            // One year of validity, the window a Windows instance's RDP
            // certificate carries in `notValidBefore` / `notValidAfter`.
            valid_from_ms: WITNESSED_AT_MS,
            valid_to_ms: WITNESSED_AT_MS + 365 * 24 * 60 * 60 * 1_000,
        }],
    },
    Row {
        case: "application blueprint, dualstack, nothing recorded yet",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "wordpress",
        bundle_id: "4xlarge_3_0",
        ip_address_type: Some("dualstack"),
        recorded: &[],
    },
];

/// Names nothing created, differing along the length, the case, the digits and
/// the separators. Every one satisfies the modelled `ResourceName` pattern
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

/// Record `recorded` as the host keys the service holds as known for the
/// instance `name`.
///
/// This is what the browser-based access path writes when it witnesses an
/// instance's host key; that path (`GetInstanceAccessDetails`) is still a 501
/// stub, so the record is written here on the instance's own record through the
/// same public state API a handler uses — writing what that operation will
/// write, so this stays correct once the stub is implemented.
fn record_known_host_keys(emulator: &Emulator, name: &str, recorded: &[Recorded]) {
    lightsail_state::with_state(emulator.engine(), |store| {
        store
            .instance_mut(name)
            .expect("the suite created this instance through CreateInstances")
            .known_host_keys = recorded
            .iter()
            .map(|record| record.attributes(WITNESSED_AT_MS))
            .collect();
    })
    .expect("the suite's engine is the shared in-memory one");
}

/// One recorded host key as the store holds it, reduced to the members that tell
/// one record from another: its algorithm, its public key and its SHA-256
/// fingerprint.
type RecordedKey = (Option<String>, Option<String>, Option<String>);

/// The host keys the store holds as known for the instance `name`, and `None`
/// when the store holds no such instance.
fn recorded_known_host_keys(emulator: &Emulator, name: &str) -> Option<Vec<RecordedKey>> {
    lightsail_state::with_state(emulator.engine(), |store| {
        store.instance(name).map(|instance| {
            instance
                .known_host_keys
                .iter()
                .map(|key| {
                    (
                        key.algorithm.clone(),
                        key.public_key.clone(),
                        key.fingerprint_sha256.clone(),
                    )
                })
                .collect()
        })
    })
    .expect("the suite's engine is the shared in-memory one")
}

/// Assert `operations` is exactly one terminal `DeleteKnownHostKeys` log entry
/// against `name`, answering its id.
fn assert_delete_operation(case: &str, operations: &Value, name: &str, region: &str) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: the call acts on one instance, so it logs one operation, got {operations:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected operationType {OPERATION_TYPE}, got {operation:?}"
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
    // The recorded keys are gone as the call returns, so the entry is answered
    // terminal rather than waiting on the clock.
    assert_eq!(
        (
            operation["status"].as_str(),
            operation["isTerminal"].as_bool()
        ),
        (Some("Succeeded"), Some(true)),
        "case {case}: expected a terminal Succeeded entry, got {operation:?}"
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

/// Every `operationType` the operation log reports for the resource `name`.
async fn logged_types_for(emulator: &Emulator, name: &str) -> Vec<String> {
    let body = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case reading the log of {name}: expected a list, got {body:?}"))
        .iter()
        .filter_map(|entry| entry["operationType"].as_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// The delete itself
// ---------------------------------------------------------------------------

/// RULE: the call empties the host keys the service holds as known for the
/// instance the request names — whatever was recorded for it, and however many —
/// leaves every other member of that instance's record where it is, and logs
/// exactly one terminal `DeleteKnownHostKeys` entry against it, readable back by
/// its id. A second call is served the same way, because what the caller asked
/// for holds whether or not anything was recorded.
#[tokio::test]
async fn deleting_known_host_keys_empties_the_record_the_service_holds() {
    for row in ROWS {
        let emulator = Emulator::new();
        let name = "browser-accessed";
        emulator.ok("CreateInstances", create_body(row, name)).await;
        wait_for_state(&emulator, name, "running").await;

        record_known_host_keys(&emulator, name, row.recorded);
        assert_eq!(
            recorded_known_host_keys(&emulator, name).as_ref().map(Vec::len),
            Some(row.recorded.len()),
            "case {}: the fixture must record {} host keys before the delete, got {:?}",
            row.case,
            row.recorded.len(),
            recorded_known_host_keys(&emulator, name)
        );

        let before = emulator
            .ok("GetInstance", json!({"instanceName": name}))
            .await["instance"]
            .clone();

        let deleted = emulator
            .ok("DeleteKnownHostKeys", json!({"instanceName": name}))
            .await;
        let first_id = assert_delete_operation(row.case, &deleted["operations"], name, row.region);

        // The record the operation is named for: nothing is known about this
        // instance's host keys any more, so the next browser-based access
        // records the key the instance presents then.
        assert_eq!(
            recorded_known_host_keys(&emulator, name),
            Some(Vec::new()),
            "case {}: the delete must leave no recorded host key, got {:?}",
            row.case,
            recorded_known_host_keys(&emulator, name)
        );

        // Everything else about the instance is untouched: this call deletes a
        // record the service holds ABOUT the instance, it does not re-provision
        // it.
        let after = emulator
            .ok("GetInstance", json!({"instanceName": name}))
            .await["instance"]
            .clone();
        assert_eq!(
            after, before,
            "case {}: the delete must leave the stored instance identical, got {after:?}",
            row.case
        );

        // The entry the call logged is the SAME record read back by its id: one
        // log, not two.
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
            "status",
            "isTerminal",
        ] {
            assert_eq!(
                read_back["operation"][member], deleted["operations"][0][member],
                "case {}: GetOperation must report the entry the delete logged, expected \
                 {member} {:?}, got {:?}",
                row.case, deleted["operations"][0][member], read_back["operation"][member]
            );
        }
        let logged = logged_types_for(&emulator, name).await;
        assert_eq!(
            logged
                .iter()
                .filter(|kind| kind.as_str() == OPERATION_TYPE)
                .count(),
            1,
            "case {}: the instance's log must hold exactly one {OPERATION_TYPE} entry, got \
             {logged:?}",
            row.case
        );

        // Again: an instance with nothing recorded is served exactly as one with
        // keys recorded was, and each call logs its own entry.
        let again = emulator
            .ok("DeleteKnownHostKeys", json!({"instanceName": name}))
            .await;
        let second_id = assert_delete_operation(
            &format!("{}, deleted twice", row.case),
            &again["operations"],
            name,
            row.region,
        );
        assert_ne!(
            first_id, second_id,
            "case {}: each call logs its own operation, got {first_id} twice",
            row.case
        );
        assert_eq!(
            recorded_known_host_keys(&emulator, name),
            Some(Vec::new()),
            "case {}: a second delete must leave the record empty too, got {:?}",
            row.case,
            recorded_known_host_keys(&emulator, name)
        );
    }
}

/// RULE: the record belongs to the ONE instance the request names, so the keys
/// recorded for every other instance are left exactly as they were — including
/// an instance whose recorded keys are identical.
#[tokio::test]
async fn deleting_known_host_keys_touches_only_the_instance_named() {
    let emulator = Emulator::new();
    let named = "named-in-the-request";
    let bystander = "left-alone";
    // The two instances differ in blueprint, bundle, Region and address type, and
    // the bystander carries the three-record fixture: neither the instance nor
    // the size of its record decides whose keys go.
    emulator
        .ok("CreateInstances", create_body(&ROWS[0], named))
        .await;
    emulator
        .ok("CreateInstances", create_body(&ROWS[1], bystander))
        .await;
    record_known_host_keys(&emulator, named, ROWS[0].recorded);
    record_known_host_keys(&emulator, bystander, ROWS[1].recorded);
    let bystander_before = recorded_known_host_keys(&emulator, bystander);

    emulator
        .ok("DeleteKnownHostKeys", json!({"instanceName": named}))
        .await;

    assert_eq!(
        recorded_known_host_keys(&emulator, named),
        Some(Vec::new()),
        "case the instance named in the request: its record must be empty, got {:?}",
        recorded_known_host_keys(&emulator, named)
    );
    assert_eq!(
        recorded_known_host_keys(&emulator, bystander),
        bystander_before,
        "case the instance not named: its recorded host keys must be untouched, expected \
         {bystander_before:?}, got {:?}",
        recorded_known_host_keys(&emulator, bystander)
    );
    // And the log says what happened to which instance.
    assert_eq!(
        logged_types_for(&emulator, bystander)
            .await
            .iter()
            .filter(|kind| kind.as_str() == OPERATION_TYPE)
            .count(),
        0,
        "case the instance not named: no {OPERATION_TYPE} entry may be logged against it"
    );
}

/// RULE: the deleted record is one the SERVICE holds about the instance, not
/// something running on it, so the instance's lifecycle state does not decide
/// whether the call is served: an instance still coming up, and one that has been
/// stopped, are served exactly as a running one is.
#[tokio::test]
async fn the_instances_lifecycle_state_does_not_decide_the_delete() {
    // `pending` is where an instance is for as long as the clock owes it
    // `running`; `stopped` is where `StopInstance` leaves it.
    for (case, stop) in [("an instance still coming up", false), ("a stopped instance", true)] {
        let emulator = Emulator::new();
        let name = "not-running";
        emulator
            .ok("CreateInstances", create_body(&ROWS[1], name))
            .await;
        if stop {
            wait_for_state(&emulator, name, "running").await;
            emulator
                .ok("StopInstance", json!({"instanceName": name, "force": true}))
                .await;
            wait_for_state(&emulator, name, "stopped").await;
        }
        record_known_host_keys(&emulator, name, ROWS[1].recorded);

        let deleted = emulator
            .ok("DeleteKnownHostKeys", json!({"instanceName": name}))
            .await;
        assert_delete_operation(case, &deleted["operations"], name, ROWS[1].region);
        assert_eq!(
            recorded_known_host_keys(&emulator, name),
            Some(Vec::new()),
            "case {case}: the delete must empty the record whatever state the instance is in, \
             got {:?}",
            recorded_known_host_keys(&emulator, name)
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: the call resolves `instanceName` against the store, so a name that was
/// never created — or has since been deleted, which takes the instance's recorded
/// host keys with it — is refused with the service's `DoesNotExist` envelope, and
/// the refusal logs nothing.
#[tokio::test]
async fn deleting_known_host_keys_of_an_unknown_instance_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    for name in MISSING_NAMES {
        let (status, body) = emulator
            .call("DeleteKnownHostKeys", json!({"instanceName": name}))
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
            "case {name:?}: expected the message to name the resource type and the requested \
             name, got {body:?}"
        );
    }

    // A deleted instance is refused exactly like one that never existed: its
    // recorded host keys went with the record.
    let name = "short-lived";
    emulator
        .ok("CreateInstances", create_body(&ROWS[2], name))
        .await;
    record_known_host_keys(&emulator, name, ROWS[2].recorded);
    emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;
    assert_eq!(
        recorded_known_host_keys(&emulator, name),
        None,
        "case after the instance is deleted: the store holds no record for it at all"
    );
    let (status, body) = emulator
        .call("DeleteKnownHostKeys", json!({"instanceName": name}))
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("DoesNotExist")),
        "case after the instance is deleted: expected the DoesNotExist envelope, got {status} \
         {body:?}"
    );

    // Nothing was logged for any of it: a refused call writes no entry.
    let log = emulator.ok("GetOperations", json!({})).await;
    let logged: Vec<&str> = log["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case after every refusal: expected a log list, got {log:?}"))
        .iter()
        .filter_map(|entry| entry["operationType"].as_str())
        .collect();
    assert!(
        !logged.contains(&OPERATION_TYPE),
        "case after every refusal: no {OPERATION_TYPE} entry may be logged, got {logged:?}"
    );
}
