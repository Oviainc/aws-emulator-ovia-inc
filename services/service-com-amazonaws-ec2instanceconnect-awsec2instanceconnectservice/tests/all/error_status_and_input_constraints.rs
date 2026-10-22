// SPDX-License-Identifier: CC0-1.0

//! How EC2 Instance Connect answers a FAILING call: with which HTTP status, with which
//! modelled error type, and which requests it refuses before it looks at any state.
//!
//! Three rules are pinned here, each over a set of inputs rather than one request:
//!
//! 1. a client-side modelled error is served with HTTP 400 whatever `@httpError` its shape
//!    carries, while a server-side one keeps the status the model gives it
//!    (`state`/`scaffolded::client_error_status`);
//! 2. a value that breaks a constraint the model declares is refused as
//!    `InvalidArgsException`, naming the member and the constraint, before any instance is
//!    looked up — including `InstanceOSUser`'s pattern, which the framework's boundary
//!    validator cannot evaluate;
//! 3. serial-console access is an account-level grant that starts absent, so a fresh
//!    account's serial-console push is refused with `SerialConsoleAccessDisabledException`
//!    before the instance registry is consulted.
//!
//! Every case drives a real `POST /` through the router and then reads the service's own
//! state back, so a canned answer could not pass. The harness (`Emulator`, `ssh_key`,
//! `error_code`, `running_instance`) is the one `instance_connect_key_push` already owns.

use std::time::Instant;

use serde_json::{json, Value};

use service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice as ec2ic;

use ec2ic::state::{Instance, InstanceState, SERIAL_CONSOLE_PORT};

use super::instance_connect_key_push::{
    error_code, running_instance, ssh_key, Emulator, SEND_SERIAL, SEND_SSH,
};

/// An instance id satisfying the modelled `InstanceId` constraints, used by every case
/// that is not about the id itself.
const INSTANCE_ID: &str = "i-0abcdef012345678";

/// What the registry holds for a case.
#[derive(Debug, Clone, Copy)]
enum Fixture {
    /// Nothing registered under the id the request names.
    Unregistered,
    /// A running instance whose type supports the serial console.
    RunningNitro,
    /// A running instance whose type predates Nitro.
    RunningPreNitro,
    /// An instance that is still launching.
    Pending,
    /// A stopped instance.
    Stopped,
}

impl Fixture {
    /// Put this fixture into `emulator`'s registry.
    fn register(self, emulator: &Emulator) {
        match self {
            Self::Unregistered => {}
            Self::RunningNitro => emulator.register(running_instance(INSTANCE_ID, "us-east-1a")),
            Self::RunningPreNitro => emulator.register(Instance::new(
                INSTANCE_ID,
                "us-east-1a",
                "m3.medium",
                InstanceState::Running,
                /* serial_console_supported */ false,
            )),
            Self::Pending => emulator
                .register(running_instance(INSTANCE_ID, "us-east-1a").in_state(InstanceState::Pending)),
            Self::Stopped => emulator
                .register(running_instance(INSTANCE_ID, "us-east-1a").in_state(InstanceState::Stopped)),
        }
    }
}

/// One row of the status table: a failure (or a success) to provoke, and the answer the
/// rule prescribes for it.
#[derive(Debug, Clone, Copy)]
struct StatusCase {
    /// What the registry holds.
    fixture: Fixture,
    /// Which operation the row calls.
    target: &'static str,
    /// An `AvailabilityZone` to add to the request, when the row needs one.
    availability_zone: Option<&'static str>,
    /// The status the emulator must answer with.
    expected_status: u16,
    /// The `__type` it must carry, or `""` for a success.
    expected_code: &'static str,
    /// The status the error shape's own `@httpError` trait declares.
    modelled_http_error: u16,
    /// The shape's `errorKind`, or `None` for the rows that succeed.
    error_kind: Option<&'static str>,
}

#[tokio::test]
async fn a_client_error_is_served_with_http_400_and_a_server_error_keeps_the_modelled_status() {
    // The `modelled_http_error` and `error_kind` columns are the model's own, from
    // `smithy_query(query="operation",operation="SendSSHPublicKey,SendSerialConsoleSSHPublicKey")`;
    // they are what the rule is stated against — a `client` shape answers 400 however far
    // its trait is from 400, a `server` shape keeps the trait's status.
    let row = |fixture,
               target,
               availability_zone,
               expected_status,
               expected_code,
               modelled_http_error,
               error_kind| StatusCase {
        fixture,
        target,
        availability_zone,
        expected_status,
        expected_code,
        modelled_http_error,
        error_kind,
    };
    let rows: [StatusCase; 10] = [
        row(
            Fixture::Unregistered,
            SEND_SSH,
            None,
            400,
            "EC2InstanceNotFoundException",
            404,
            Some("client"),
        ),
        row(
            Fixture::Unregistered,
            SEND_SERIAL,
            None,
            400,
            "EC2InstanceNotFoundException",
            404,
            Some("client"),
        ),
        row(
            Fixture::Stopped,
            SEND_SSH,
            None,
            400,
            "EC2InstanceStateInvalidException",
            400,
            Some("client"),
        ),
        row(
            Fixture::Stopped,
            SEND_SERIAL,
            None,
            400,
            "EC2InstanceStateInvalidException",
            400,
            Some("client"),
        ),
        row(
            Fixture::RunningPreNitro,
            SEND_SERIAL,
            None,
            400,
            "EC2InstanceTypeInvalidException",
            400,
            Some("client"),
        ),
        // A zone that is not the instance's: a parameter that is not valid.
        row(
            Fixture::RunningNitro,
            SEND_SSH,
            Some("us-east-1b"),
            400,
            "InvalidArgsException",
            400,
            Some("client"),
        ),
        // Still launching is a failure this service classifies as a SERVER fault, and it
        // keeps 503 — the rule moves client errors only.
        row(
            Fixture::Pending,
            SEND_SSH,
            None,
            503,
            "EC2InstanceUnavailableException",
            503,
            Some("server"),
        ),
        row(
            Fixture::Pending,
            SEND_SERIAL,
            None,
            503,
            "EC2InstanceUnavailableException",
            503,
            Some("server"),
        ),
        row(Fixture::RunningNitro, SEND_SSH, None, 200, "", 200, None),
        row(Fixture::RunningNitro, SEND_SERIAL, None, 200, "", 200, None),
    ];

    for case in rows {
        let StatusCase {
            fixture,
            target,
            availability_zone,
            expected_status,
            expected_code,
            modelled_http_error,
            error_kind,
        } = case;
        let case = format!(
            "{target} against {fixture:?} (modelled @httpError {modelled_http_error}, \
             errorKind {error_kind:?})"
        );
        let emulator = Emulator::new();
        // The account grant is not what these rows are about.
        emulator.enable_serial_console();
        fixture.register(&emulator);

        let mut body = if target == SEND_SSH {
            json!({
                "InstanceId": INSTANCE_ID,
                "InstanceOSUser": "ec2-user",
                "SSHPublicKey": ssh_key("status"),
            })
        } else {
            json!({
                "InstanceId": INSTANCE_ID,
                "SerialPort": SERIAL_CONSOLE_PORT,
                "SSHPublicKey": ssh_key("status"),
            })
        };
        if let Some(zone) = availability_zone {
            body["AvailabilityZone"] = json!(zone);
        }

        let (status, response) = emulator.call(target, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({response})"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected __type {expected_code:?}, got {response}"
        );
        if error_kind == Some("client") {
            assert_eq!(
                status, 400,
                "case {case:?}: every client error of this service is served as 400, got \
                 {status}"
            );
        }
        if error_kind == Some("server") {
            assert_eq!(
                status, modelled_http_error,
                "case {case:?}: a server error keeps the status the model declares, got \
                 {status}"
            );
        }
        let expected_stored = usize::from(expected_status == 200);
        assert_eq!(
            emulator.stored_key_count(),
            expected_stored,
            "case {case:?}: expected {expected_stored} stored key(s), state holds {}",
            emulator.stored_key_count()
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_as_invalid_args() {
    // (case, target, request body, the member the clause must name, the requirement clause
    // it must state). Bounds and patterns are the model's own, from
    // `smithy_query(query="operation",…)`: `InstanceId` len 10..32 pattern ^i-[a-f0-9]+$,
    // `InstanceOSUser` len 1..32 plus its pattern, `SSHPublicKey` len 80..4096,
    // `AvailabilityZone` len 6..32 pattern ^(\w+-){2,3}\d+\w+$. A member names itself
    // lowerCamel in the clause, which is why `SSHPublicKey` appears as `sSHPublicKey`, and a
    // pattern is stated without the anchors that bracket it (see `state::as_stated`).
    let valid_key = ssh_key("constraint");
    let rows: [(&str, &str, Value, &str, &str); 12] = [
        (
            "InstanceId one character under the 10-character minimum",
            SEND_SSH,
            json!({"InstanceId": "i-0123456", "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key}),
            "instanceId",
            "Member must have length greater than or equal to 10",
        ),
        (
            "InstanceId one character over the 32-character maximum",
            SEND_SSH,
            json!({"InstanceId": format!("i-{}", "a".repeat(31)), "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key}),
            "instanceId",
            "Member must have length less than or equal to 32",
        ),
        (
            "InstanceId with a digit outside hex",
            SEND_SERIAL,
            json!({"InstanceId": "i-0123456789g", "SerialPort": 0, "SSHPublicKey": valid_key}),
            "instanceId",
            "Member must satisfy regular expression pattern: i-[a-f0-9]+",
        ),
        (
            "InstanceOSUser empty, under the 1-character minimum",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "", "SSHPublicKey": valid_key}),
            "instanceOSUser",
            "Member must have length greater than or equal to 1",
        ),
        (
            "InstanceOSUser one character over the 32-character maximum",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "a".repeat(33), "SSHPublicKey": valid_key}),
            "instanceOSUser",
            "Member must have length less than or equal to 32",
        ),
        (
            "InstanceOSUser with spaces and punctuation the pattern forbids",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "not a valid user!!", "SSHPublicKey": valid_key}),
            "instanceOSUser",
            "Member must satisfy regular expression pattern:",
        ),
        (
            // Long enough for `@length` (1..32) and only the pattern's second branch could
            // admit it — and that branch's lookahead requires at least two characters.
            "InstanceOSUser a single digit",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "1", "SSHPublicKey": valid_key}),
            "instanceOSUser",
            "Member must satisfy regular expression pattern:",
        ),
        (
            "SSHPublicKey one character under the 80-character minimum",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "ec2-user", "SSHPublicKey": "k".repeat(79)}),
            "sSHPublicKey",
            "Member must have length greater than or equal to 80",
        ),
        (
            "SSHPublicKey one character over the 4096-character maximum",
            SEND_SERIAL,
            json!({"InstanceId": INSTANCE_ID, "SerialPort": 0, "SSHPublicKey": "k".repeat(4097)}),
            "sSHPublicKey",
            "Member must have length less than or equal to 4096",
        ),
        (
            "AvailabilityZone one character under the 6-character minimum",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key, "AvailabilityZone": "us-1a"}),
            "availabilityZone",
            "Member must have length greater than or equal to 6",
        ),
        (
            "AvailabilityZone that is not a zone name",
            SEND_SSH,
            json!({"InstanceId": INSTANCE_ID, "InstanceOSUser": "ec2-user", "SSHPublicKey": valid_key, "AvailabilityZone": "not-a-zone"}),
            "availabilityZone",
            r"Member must satisfy regular expression pattern: (\w+-){2,3}\d+\w+",
        ),
        (
            "SerialPort outside the modelled range 0..0",
            SEND_SERIAL,
            json!({"InstanceId": INSTANCE_ID, "SerialPort": 1, "SSHPublicKey": valid_key}),
            "serialPort",
            "Member must have value less than or equal to 0",
        ),
    ];

    for (case, target, body, member, requirement) in rows {
        let emulator = Emulator::new();
        emulator.enable_serial_console();
        // Registered and able to take a key, so the only thing wrong is the constraint.
        emulator.register(running_instance(INSTANCE_ID, "us-east-1a"));

        let (status, response) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({response})"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgsException",
            "case {case:?}: expected InvalidArgsException, got {response}"
        );
        let message = response
            .get("Message")
            .or_else(|| response.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("validation error") && message.contains("detected:"),
            "case {case:?}: expected the validation envelope, got {message:?}"
        );
        assert!(
            message.contains(&format!("at '{member}'")),
            "case {case:?}: expected the clause to name '{member}', got {message:?}"
        );
        assert!(
            message.contains(requirement),
            "case {case:?}: expected the clause to state {requirement:?}, got {message:?}"
        );
        if let Some((_, stated)) = message.split_once("regular expression pattern: ") {
            // The clause names the expression, not the anchors that bracket it.
            assert!(
                !stated.starts_with('^') && !stated.trim_end().ends_with('$'),
                "case {case:?}: the stated pattern must carry no bracketing anchors, got \
                 {stated:?}"
            );
        }
        assert_eq!(
            emulator.stored_key_count(),
            0,
            "case {case:?}: a refused request must store nothing"
        );
    }
}

#[tokio::test]
async fn the_os_user_values_the_modelled_pattern_admits_are_the_ones_accepted() {
    // The catalog's source is `InstanceOSUser`'s own `@pattern`, an alternation of
    // `^[A-Za-z_][A-Za-z0-9@._-]{0,30}[A-Za-z0-9$_-]?$` (a name opening with a letter or an
    // underscore) and `^(?=.{2,32}$)[0-9]{1,32}[A-Za-z@._-][A-Za-z0-9@._-]*[A-Za-z0-9$_-]?$`
    // (a name opening with digits, at least two characters long). Rows span both branches
    // and the characters each admits; none of them is the value the issue cited.
    let rows: [(&str, bool); 14] = [
        ("ec2-user", true),
        ("ubuntu", true),
        ("a", true),
        ("_svc", true),
        ("A.b@c-d", true),
        ("user_1$", true),
        (&"a".repeat(32), true),
        // The second branch: digits first, then a letter.
        ("0user", true),
        ("1a", true),
        // Refused: the first branch needs a letter or underscore first, and the second
        // needs a letter after its digits and at least two characters in all.
        ("1", false),
        ("12", false),
        ("-user", false),
        ("user!", false),
        ("ec2 user", false),
    ];

    for (os_user, accepted) in rows {
        let case = format!("InstanceOSUser {os_user:?}");
        let emulator = Emulator::new();
        emulator.register(running_instance(INSTANCE_ID, "us-east-1a"));

        let key = ssh_key("os-user");
        let (status, response) = emulator.push_ssh(INSTANCE_ID, os_user, &key).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the pattern admits it, so expected HTTP 200, got {status} \
                 ({response})"
            );
            let state = emulator.state();
            let guard = state.lock();
            let stored = guard
                .active_ssh_key(INSTANCE_ID, os_user, Instant::now())
                .map(|stored| stored.public_key.clone());
            assert_eq!(
                stored.as_deref(),
                Some(key.as_str()),
                "case {case:?}: expected the key stored under that OS user, got {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the pattern refuses it, so expected HTTP 400, got {status} \
                 ({response})"
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
async fn serial_console_access_starts_absent_and_is_decided_before_the_instance_is_looked_up() {
    // Two ids: one registered and able to take a key, one that is not in the registry at
    // all. Neither can matter while the account has no serial-console grant.
    let unregistered = "i-0fedcba987654321f";
    for instance_id in [INSTANCE_ID, unregistered] {
        let case = format!("fresh account, serial-console push for {instance_id}");
        let emulator = Emulator::new();
        emulator.register(running_instance(INSTANCE_ID, "us-east-1a"));
        assert!(
            emulator.state().lock().instance(unregistered).is_none(),
            "case {case:?}: the second id must not be registered"
        );

        let (status, response) = emulator.push_serial(instance_id, &ssh_key("grant")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({response})"
        );
        assert_eq!(
            error_code(&response),
            "SerialConsoleAccessDisabledException",
            "case {case:?}: expected SerialConsoleAccessDisabledException, got {response}"
        );
        let message = response
            .get("Message")
            .or_else(|| response.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(engine::EMULATOR_ACCOUNT_ID)
                && message.contains("EnableSerialConsoleAccess"),
            "case {case:?}: the refusal must name the account it was made under and the API \
             that grants access, got {message:?}"
        );
        assert_eq!(
            emulator.stored_key_count(),
            0,
            "case {case:?}: a refused push must store nothing"
        );

        // The plain SSH push does not read the serial-console grant, so it still works.
        let (status, response) = emulator
            .push_ssh(INSTANCE_ID, "ec2-user", &ssh_key("grant-ssh"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SendSSHPublicKey does not read the grant, got {status} \
             ({response})"
        );
    }

    // Once the account is granted access (what EC2's `EnableSerialConsoleAccess` does), the
    // same push succeeds and the key is readable from state; an unknown instance then gets
    // the not-found answer, which is the next decision in order.
    let emulator = Emulator::new();
    emulator.register(running_instance(INSTANCE_ID, "us-east-1a"));
    emulator.enable_serial_console();

    let key = ssh_key("granted");
    let (status, response) = emulator.push_serial(INSTANCE_ID, &key).await;
    assert_eq!(
        status, 200,
        "with the account granted access the push must succeed, got {status} ({response})"
    );
    {
        let state = emulator.state();
        let guard = state.lock();
        let stored = guard
            .active_serial_console_key(INSTANCE_ID, SERIAL_CONSOLE_PORT, Instant::now())
            .map(|stored| stored.public_key.clone());
        assert_eq!(
            stored.as_deref(),
            Some(key.as_str()),
            "the granted push must store the key under port {SERIAL_CONSOLE_PORT}, got \
             {stored:?}"
        );
    }

    let (status, response) = emulator.push_serial(unregistered, &ssh_key("granted")).await;
    assert_eq!(
        status, 400,
        "with access granted an unknown instance is a 400, got {status} ({response})"
    );
    assert_eq!(
        error_code(&response),
        "EC2InstanceNotFoundException",
        "with access granted an unknown instance is not found, got {response}"
    );
}
