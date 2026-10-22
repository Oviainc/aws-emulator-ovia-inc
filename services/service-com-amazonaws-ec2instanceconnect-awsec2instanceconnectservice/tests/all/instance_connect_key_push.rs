// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for both EC2 Instance Connect operations.
//!
//! Every case drives a real `POST /` with an `X-Amz-Target` through the multi-service
//! router — the same path a signed SDK request takes — and then reads the service's own
//! state back, so a canned `Success: true` could not pass any of them.
//!
//! The tables below span the classes the rules generalise over: every lifecycle state an
//! instance can be in, Availability Zones that match and do not match across several
//! regions, instance ids that are not registered, the key lifetime at and past its bound,
//! and the modelled input constraints.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice as ec2ic;

use ec2ic::state::{
    ComAmazonawsEc2instanceconnectAwsec2instanceconnectserviceState as IcState, Instance,
    InstanceState, PUSHED_KEY_TTL, SERIAL_CONSOLE_PORT,
};

/// The `X-Amz-Target` values from this service's wire protocol (`OPERATIONS.md`:
/// `AWSEC2InstanceConnectService.<OperationName>`).
const SEND_SSH: &str = "AWSEC2InstanceConnectService.SendSSHPublicKey";
const SEND_SERIAL: &str = "AWSEC2InstanceConnectService.SendSerialConsoleSSHPublicKey";

/// A SigV4 credential scope naming this service's signing name (`ec2-instance-connect`),
/// which is how the router picks the service out (`resolve_route_key` reads the scope's
/// service field first). The harness switches request authentication off, so the signature
/// itself is never verified and need not be real.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ec2-instance-connect/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=00";

/// Key material satisfying the modelled `SSHPublicKey` length (80..4096); `tag` makes each
/// one distinct, so a replacement is visible in state.
fn ssh_key(tag: &str) -> String {
    format!("ssh-ed25519 {} {tag}@emulator", "A".repeat(80))
}

/// A `RequestId` is a UUID: 8-4-4-4-12 hex digits.
fn is_uuid(value: &str) -> bool {
    let group_lengths: Vec<usize> = value.split('-').map(str::len).collect();
    group_lengths.as_slice() == [8usize, 4, 4, 4, 12]
        && value.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// The error code an awsJson1_1 error envelope carries in `__type`, or `""` for a body with
/// no `__type` (a success).
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One emulator: a shared engine, and the router with only this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(ec2ic::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store in the shared engine — reached exactly as a handler reaches it.
    fn state(&self) -> engine::ServiceState<IcState> {
        ec2ic::state::service_state(&*self.engine).expect("the service state must be reachable")
    }

    fn register(&self, instance: Instance) {
        self.state().lock().register_instance(instance);
    }

    /// `POST /` with `X-Amz-Target: <target>` and a JSON body, through the router.
    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "ec2-instance-connect.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header("authorization", AUTHORIZATION)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn push_ssh(&self, instance_id: &str, os_user: &str, key: &str) -> (u16, Value) {
        self.call(
            SEND_SSH,
            &json!({
                "InstanceId": instance_id,
                "InstanceOSUser": os_user,
                "SSHPublicKey": key,
            }),
        )
        .await
    }

    async fn push_serial(&self, instance_id: &str, key: &str) -> (u16, Value) {
        self.call(
            SEND_SERIAL,
            &json!({
                "InstanceId": instance_id,
                "SerialPort": SERIAL_CONSOLE_PORT,
                "SSHPublicKey": key,
            }),
        )
        .await
    }

    /// How many keys of either kind are stored right now, expiry ignored.
    fn stored_key_count(&self) -> usize {
        let state = self.state();
        let guard = state.lock();
        guard.pushed_keys.len() + guard.serial_console_keys.len()
    }
}

/// A running Nitro instance a case can push to.
fn running_instance(instance_id: &str, zone: &str) -> Instance {
    Instance::new(
        instance_id,
        zone,
        "m5.large",
        InstanceState::Running,
        /* serial_console_supported */ true,
    )
}

#[tokio::test]
async fn a_push_stores_the_key_against_the_instance_and_user_and_a_second_push_replaces_it() {
    let emulator = Emulator::new();
    let instance = running_instance("i-0abcdef012345678", "us-east-1a");
    emulator.register(instance.clone());

    let first = ssh_key("first");
    let (status, body) = emulator
        .push_ssh(&instance.instance_id, "ec2-user", &first)
        .await;
    assert_eq!(status, 200, "first push: expected 200, got {status} ({body})");
    assert_eq!(
        body.get("Success").and_then(Value::as_bool),
        Some(true),
        "first push: expected Success true, got {body}"
    );
    let first_request_id = body
        .get("RequestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_uuid(&first_request_id),
        "first push: expected a UUID RequestId, got {first_request_id:?}"
    );

    // Read the key back out of the service's own state.
    let (stored_key, stored_request_id, first_pushed_at, first_expires_at) = {
        let state = emulator.state();
        let guard = state.lock();
        let key = guard
            .active_ssh_key(&instance.instance_id, "ec2-user", Instant::now())
            .expect("first push: the key must be readable from state");
        (
            key.public_key.clone(),
            key.request_id.clone(),
            key.pushed_at,
            key.expires_at,
        )
    };
    assert_eq!(
        stored_key, first,
        "first push: expected the pushed key material in state, got {stored_key:?}"
    );
    assert_eq!(
        stored_request_id, first_request_id,
        "first push: the stored record must carry the RequestId that was returned, got \
         {stored_request_id:?}"
    );
    assert_eq!(
        first_expires_at.duration_since(first_pushed_at),
        PUSHED_KEY_TTL,
        "first push: expected the key to expire {PUSHED_KEY_TTL:?} after it was pushed"
    );

    // A second push for the same (instance, user) replaces the key and refreshes the expiry.
    let second = ssh_key("second");
    let (status, body) = emulator
        .push_ssh(&instance.instance_id, "ec2-user", &second)
        .await;
    assert_eq!(
        status, 200,
        "second push: expected 200, got {status} ({body})"
    );
    let second_request_id = body
        .get("RequestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        second_request_id, first_request_id,
        "second push: expected a fresh RequestId, got {second_request_id:?} again"
    );

    // A handler takes the same state lock, so each read of state is scoped: a guard still
    // alive at the next call would deadlock it.
    {
        let state = emulator.state();
        let guard = state.lock();
        assert_eq!(
            guard.pushed_keys.len(),
            1,
            "second push: expected the prior key to be replaced, state holds {} keys",
            guard.pushed_keys.len()
        );
        let key = guard
            .active_ssh_key(&instance.instance_id, "ec2-user", Instant::now())
            .expect("second push: the key must be readable from state");
        assert_eq!(
            key.public_key, second,
            "second push: expected the replacing key material, got {:?}",
            key.public_key
        );
        assert_eq!(
            key.request_id, second_request_id,
            "second push: expected the stored record to carry the second RequestId, got {:?}",
            key.request_id
        );
        assert!(
            key.expires_at > first_expires_at,
            "second push: expected a refreshed expiry, got {:?} which is not after {:?}",
            key.expires_at,
            first_expires_at
        );
    }

    // The same key material pushed for a DIFFERENT OS user is a separate record.
    let (status, _) = emulator
        .push_ssh(&instance.instance_id, "ubuntu", &ssh_key("other-user"))
        .await;
    assert_eq!(status, 200, "push for a second OS user: expected 200");
    let state = emulator.state();
    let guard = state.lock();
    assert_eq!(
        guard.pushed_keys.len(),
        2,
        "a push for another OS user must not replace the first user's key; state holds {} keys",
        guard.pushed_keys.len()
    );
}

#[tokio::test]
async fn the_instance_lifecycle_state_decides_the_outcome_of_both_operations() {
    // Every state EC2 reports, with the outcome the modelled errors prescribe: a launching
    // instance is "currently unavailable" (503), one that is going down, stopped or
    // terminated is an invalid state (400), and only a running one takes a key.
    let rows = [
        (InstanceState::Running, 200u16, ""),
        (
            InstanceState::Pending,
            503,
            "EC2InstanceUnavailableException",
        ),
        (
            InstanceState::Stopping,
            400,
            "EC2InstanceStateInvalidException",
        ),
        (
            InstanceState::Stopped,
            400,
            "EC2InstanceStateInvalidException",
        ),
        (
            InstanceState::Terminated,
            400,
            "EC2InstanceStateInvalidException",
        ),
    ];

    for (state, expected_status, expected_code) in rows {
        for target in [SEND_SSH, SEND_SERIAL] {
            let case = format!("{} on a {} instance", target, state.as_str());
            let emulator = Emulator::new();
            let instance_id = "i-0fedcba987654321";
            emulator.register(
                running_instance(instance_id, "eu-west-1b").in_state(state),
            );

            let key = ssh_key("lifecycle");
            let (status, body) = if target == SEND_SSH {
                emulator.push_ssh(instance_id, "ec2-user", &key).await
            } else {
                emulator.push_serial(instance_id, &key).await
            };

            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} ({body})"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected error code {expected_code:?}, got {body}"
            );
            let expected_stored = usize::from(expected_status == 200);
            assert_eq!(
                emulator.stored_key_count(),
                expected_stored,
                "case {case:?}: expected {expected_stored} stored key(s) after the call"
            );
        }
    }
}

#[tokio::test]
async fn an_availability_zone_is_accepted_only_when_it_is_the_instances_own() {
    // (zone the instance was launched in, zone the request names, does it succeed)
    // The zone names span the forms the modelled `AvailabilityZone` pattern
    // (^(\w+-){2,3}\d+\w+$) admits, including a three-segment partition name.
    let rows = [
        ("us-east-1a", None, true),
        ("us-east-1a", Some("us-east-1a"), true),
        ("us-east-1a", Some("us-east-1b"), false),
        ("eu-west-1b", None, true),
        ("eu-west-1b", Some("eu-west-1b"), true),
        ("eu-west-1b", Some("us-east-1b"), false),
        ("ap-northeast-1d", Some("ap-northeast-1d"), true),
        ("ap-northeast-1d", Some("ap-northeast-1a"), false),
        ("us-gov-west-1a", Some("us-gov-west-1a"), true),
        ("us-gov-west-1a", Some("us-gov-west-1b"), false),
    ];

    for (instance_zone, requested_zone, should_succeed) in rows {
        let case = format!("instance in {instance_zone}, request names {requested_zone:?}");
        let emulator = Emulator::new();
        let instance_id = "i-0123456789abcdef";
        emulator.register(running_instance(instance_id, instance_zone));

        let key = ssh_key("zone");
        let mut body = json!({
            "InstanceId": instance_id,
            "InstanceOSUser": "ec2-user",
            "SSHPublicKey": key,
        });
        if let Some(zone) = requested_zone {
            body["AvailabilityZone"] = json!(zone);
        }
        let (status, response) = emulator.call(SEND_SSH, &body).await;

        if should_succeed {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} ({response})"
            );
            assert_eq!(
                emulator.stored_key_count(),
                1,
                "case {case:?}: the key must be stored on success"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({response})"
            );
            assert_eq!(
                error_code(&response),
                "InvalidArgsException",
                "case {case:?}: expected InvalidArgsException, got {response}"
            );
            assert_eq!(
                emulator.stored_key_count(),
                0,
                "case {case:?}: a refused push must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_push_to_an_unregistered_instance_is_not_found() {
    // Ids that satisfy the modelled `InstanceId` pattern and length but name no instance.
    let unregistered = [
        "i-00000000deadbeef",
        "i-0000000000",
        "i-abcdefabcdefabcdefabcdefabcdef",
    ];

    for instance_id in unregistered {
        for target in [SEND_SSH, SEND_SERIAL] {
            let case = format!("{target} for {instance_id}");
            let emulator = Emulator::new();
            // The seeded fleet must not contain it, or the case would prove nothing.
            assert!(
                emulator.state().lock().instance(instance_id).is_none(),
                "case {case:?}: the id must not be registered"
            );

            let key = ssh_key("missing");
            let (status, body) = if target == SEND_SSH {
                emulator.push_ssh(instance_id, "ec2-user", &key).await
            } else {
                emulator.push_serial(instance_id, &key).await
            };

            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} ({body})"
            );
            assert_eq!(
                error_code(&body),
                "EC2InstanceNotFoundException",
                "case {case:?}: expected EC2InstanceNotFoundException, got {body}"
            );
            assert_eq!(
                emulator.stored_key_count(),
                0,
                "case {case:?}: a refused push must store nothing"
            );
        }
    }
}

#[tokio::test]
async fn a_pushed_key_lives_for_its_ttl_and_then_reads_as_absent() {
    let emulator = Emulator::new();
    let instance_id = "i-0aaaabbbbccccdddd";
    emulator.register(running_instance(instance_id, "us-east-1a"));

    let (status, _) = emulator
        .push_ssh(instance_id, "ec2-user", &ssh_key("ttl"))
        .await;
    assert_eq!(status, 200, "the SSH push must succeed");
    let (status, _) = emulator.push_serial(instance_id, &ssh_key("ttl")).await;
    assert_eq!(status, 200, "the serial-console push must succeed");

    let state = emulator.state();
    let (ssh_pushed_at, serial_pushed_at) = {
        let guard = state.lock();
        let ssh = guard
            .active_ssh_key(instance_id, "ec2-user", Instant::now())
            .expect("the SSH key must be in state");
        let serial = guard
            .active_serial_console_key(instance_id, SERIAL_CONSOLE_PORT, Instant::now())
            .expect("the serial-console key must be in state");
        assert_eq!(
            ssh.expires_at.duration_since(ssh.pushed_at),
            PUSHED_KEY_TTL,
            "the SSH key must expire {PUSHED_KEY_TTL:?} after it was pushed"
        );
        assert_eq!(
            serial.expires_at.duration_since(serial.pushed_at),
            PUSHED_KEY_TTL,
            "the serial-console key must expire {PUSHED_KEY_TTL:?} after it was pushed"
        );
        // The remaining lifetime shrinks with the clock and floors at zero.
        for (offset, expected_remaining) in [
            (Duration::ZERO, PUSHED_KEY_TTL),
            (Duration::from_secs(10), PUSHED_KEY_TTL - Duration::from_secs(10)),
            (PUSHED_KEY_TTL, Duration::ZERO),
            (PUSHED_KEY_TTL + Duration::from_secs(5), Duration::ZERO),
        ] {
            let remaining = ssh.remaining_at(ssh.pushed_at + offset);
            assert_eq!(
                remaining, expected_remaining,
                "case {offset:?} after the push: expected {expected_remaining:?} of the key's \
                 lifetime left, got {remaining:?}"
            );
        }
        (ssh.pushed_at, serial.pushed_at)
    };

    // Inside the lifetime the key is there; from the bound onwards it reads as absent — the
    // lookup applies the cutoff whether or not anything has pruned it yet. Each key is
    // measured from its OWN push instant, since the two pushes are separate calls.
    let offsets: [(Duration, bool); 6] = [
        (Duration::ZERO, true),
        (Duration::from_secs(1), true),
        (PUSHED_KEY_TTL - Duration::from_millis(1), true),
        (PUSHED_KEY_TTL, false),
        (PUSHED_KEY_TTL + Duration::from_secs(1), false),
        (Duration::from_secs(3600), false),
    ];
    for (offset, expected_present) in offsets {
        let guard = state.lock();
        let ssh_present = guard
            .active_ssh_key(instance_id, "ec2-user", ssh_pushed_at + offset)
            .is_some();
        let serial_present = guard
            .active_serial_console_key(
                instance_id,
                SERIAL_CONSOLE_PORT,
                serial_pushed_at + offset,
            )
            .is_some();
        assert_eq!(
            ssh_present, expected_present,
            "case {offset:?} after the push: expected the SSH key present={expected_present}, \
             got {ssh_present}"
        );
        assert_eq!(
            serial_present, expected_present,
            "case {offset:?} after the push: expected the serial-console key \
             present={expected_present}, got {serial_present}"
        );
    }

    // And the expired records are dropped once the clock has made them due.
    state
        .lock()
        .drop_expired_keys(serial_pushed_at + PUSHED_KEY_TTL);
    assert_eq!(
        emulator.stored_key_count(),
        0,
        "expired keys must be gone from state once pruned"
    );
}

#[tokio::test]
async fn the_serial_console_push_reads_the_account_toggle_and_the_instance_type() {
    // (serial console enabled for the account, instance type supports it, status, code)
    let rows = [
        (true, true, 200u16, ""),
        (
            true,
            false,
            400,
            "EC2InstanceTypeInvalidException",
        ),
        (
            false,
            true,
            403,
            "SerialConsoleAccessDisabledException",
        ),
        // Access disabled is decided before the instance type, so it is what a caller sees
        // when both are wrong.
        (
            false,
            false,
            403,
            "SerialConsoleAccessDisabledException",
        ),
    ];

    for (access_enabled, type_supported, expected_status, expected_code) in rows {
        let case =
            format!("serial console access={access_enabled}, type supported={type_supported}");
        let emulator = Emulator::new();
        let instance_id = "i-0deadbeef0000cafe";
        emulator.register(Instance::new(
            instance_id,
            "us-east-1a",
            if type_supported { "c6i.large" } else { "m3.medium" },
            InstanceState::Running,
            type_supported,
        ));
        emulator.state().lock().serial_console_access_enabled = access_enabled;

        let key = ssh_key("serial");
        let (status, body) = emulator.push_serial(instance_id, &key).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({body})"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code:?}, got {body}"
        );

        {
            let state = emulator.state();
            let guard = state.lock();
            let stored = guard.active_serial_console_key(
                instance_id,
                SERIAL_CONSOLE_PORT,
                Instant::now(),
            );
            if expected_status == 200 {
                let stored = stored.expect("case: a successful push must store the key");
                assert_eq!(
                    stored.public_key, key,
                    "case {case:?}: expected the pushed key material under port \
                     {SERIAL_CONSOLE_PORT}, got {:?}",
                    stored.public_key
                );
                assert_eq!(
                    stored.os_user, None,
                    "case {case:?}: a serial-console key has no OS user, got {:?}",
                    stored.os_user
                );
            } else {
                assert!(
                    stored.is_none(),
                    "case {case:?}: a refused push must store nothing"
                );
            }
        }

        // Neither condition touches `SendSSHPublicKey`: it is a different door onto the same
        // running instance.
        let (status, body) = emulator
            .push_ssh(instance_id, "ec2-user", &ssh_key("plain"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SendSSHPublicKey must still succeed, got {status} ({body})"
        );
    }
}

#[tokio::test]
async fn every_call_gets_its_own_request_id_and_is_recorded() {
    let emulator = Emulator::new();
    let instance_id = "i-0f0f0f0f0f0f0f0f";
    emulator.register(running_instance(instance_id, "us-east-1a"));

    let mut returned: Vec<(String, String)> = Vec::new();
    for round in 0..4 {
        for target in [SEND_SSH, SEND_SERIAL] {
            let key = ssh_key(&format!("round-{round}"));
            let (status, body) = if target == SEND_SSH {
                emulator.push_ssh(instance_id, "ec2-user", &key).await
            } else {
                emulator.push_serial(instance_id, &key).await
            };
            assert_eq!(
                status, 200,
                "round {round} of {target}: expected 200, got {status} ({body})"
            );
            let request_id = body
                .get("RequestId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                is_uuid(&request_id),
                "round {round} of {target}: expected a UUID RequestId, got {request_id:?}"
            );
            let operation = target
                .split_once('.')
                .map(|(_, op)| op.to_string())
                .expect("the target names an operation");
            returned.push((operation, request_id));
        }
    }

    let distinct: BTreeSet<&String> = returned.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        returned.len(),
        "expected {} distinct RequestIds, got {}",
        returned.len(),
        distinct.len()
    );

    let state = emulator.state();
    let guard = state.lock();
    let recorded: Vec<(String, String)> = guard
        .requests
        .iter()
        .map(|record| (record.operation.clone(), record.request_id.clone()))
        .collect();
    assert_eq!(
        recorded, returned,
        "every served call must be recorded, in order, with the id it answered with"
    );
    assert!(
        guard
            .requests
            .iter()
            .all(|record| record.instance_id == instance_id),
        "every record must name the instance the call was for"
    );
}

#[tokio::test]
async fn the_modelled_input_constraints_are_refused_before_anything_is_stored() {
    // Each row violates one modelled constraint, so the boundary validator refuses it and no
    // handler runs. Source: `OPERATIONS.md` — `InstanceId` (len 10..32, pattern
    // ^i-[a-f0-9]+$), `InstanceOSUser` (len 1..32), `SSHPublicKey` (len 80..4096),
    // `SerialPort` (range 0..0).
    let valid_key = ssh_key("constraint");
    let rows: [(&str, &str, Value); 6] = [
        (
            "InstanceId shorter than 10",
            SEND_SSH,
            json!({"InstanceId": "i-0a", "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key}),
        ),
        (
            "InstanceId that is not hex",
            SEND_SSH,
            json!({"InstanceId": "i-00xyz00xyz", "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key}),
        ),
        (
            "InstanceOSUser empty",
            SEND_SSH,
            json!({"InstanceId": "i-0abcdef012345678", "InstanceOSUser": "", "SSHPublicKey": valid_key}),
        ),
        (
            "SSHPublicKey shorter than 80",
            SEND_SSH,
            json!({"InstanceId": "i-0abcdef012345678", "InstanceOSUser": "ec2-user", "SSHPublicKey": "ssh-ed25519 AAAA"}),
        ),
        (
            "SerialPort above the modelled range",
            SEND_SERIAL,
            json!({"InstanceId": "i-0abcdef012345678", "SerialPort": 1, "SSHPublicKey": valid_key}),
        ),
        (
            "AvailabilityZone that is not a zone name",
            SEND_SSH,
            json!({"InstanceId": "i-0abcdef012345678", "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key, "AvailabilityZone": "zone!"}),
        ),
    ];

    for (case, target, body) in rows {
        let emulator = Emulator::new();
        // Registered, so the only thing wrong with the request is the constraint.
        emulator.register(running_instance("i-0abcdef012345678", "us-east-1a"));

        let (status, response) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({response})"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the boundary validator's ValidationException, got {response}"
        );
        assert_eq!(
            emulator.stored_key_count(),
            0,
            "case {case:?}: a refused request must store nothing"
        );
    }
}

#[tokio::test]
async fn the_seeded_registry_serves_a_push_with_no_test_setup() {
    let emulator = Emulator::new();
    let seeded = ec2ic::state::seed_instances();

    // The seed exists so a client that never touched state can still reach an instance.
    let running = seeded
        .iter()
        .find(|instance| {
            instance.state == InstanceState::Running && instance.serial_console_supported
        })
        .expect("the seed must include a running instance that supports the serial console");
    let (status, body) = emulator
        .push_ssh(&running.instance_id, "ec2-user", &ssh_key("seeded"))
        .await;
    assert_eq!(
        status, 200,
        "a push to the seeded running instance must succeed, got {status} ({body})"
    );
    let (status, body) = emulator
        .push_serial(&running.instance_id, &ssh_key("seeded-serial"))
        .await;
    assert_eq!(
        status, 200,
        "a serial-console push to the seeded running instance must succeed, got {status} \
         ({body})"
    );
    assert_eq!(
        emulator.stored_key_count(),
        2,
        "both seeded pushes must be stored"
    );

    let stopped = seeded
        .iter()
        .find(|instance| instance.state == InstanceState::Stopped)
        .expect("the seed must include a stopped instance");
    let (status, body) = emulator
        .push_ssh(&stopped.instance_id, "ec2-user", &ssh_key("seeded-stopped"))
        .await;
    assert_eq!(
        status, 400,
        "a push to the seeded stopped instance must be refused, got {status} ({body})"
    );
    assert_eq!(
        error_code(&body),
        "EC2InstanceStateInvalidException",
        "a push to the seeded stopped instance must report the invalid state, got {body}"
    );

    // Stopping and starting that instance is the registry's own state, and the outcome
    // follows it.
    assert!(
        emulator
            .state()
            .lock()
            .set_instance_state(&stopped.instance_id, InstanceState::Running),
        "the seeded instance must be in the registry"
    );
    let (status, body) = emulator
        .push_ssh(&stopped.instance_id, "ec2-user", &ssh_key("seeded-started"))
        .await;
    assert_eq!(
        status, 200,
        "once running, the same instance must take a key, got {status} ({body})"
    );
}
