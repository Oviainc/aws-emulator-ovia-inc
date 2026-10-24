// SPDX-License-Identifier: CC0-1.0

//! `StartInputDevice` and `StopInputDevice` — the run state of an input device,
//! end to end over the restJson1 wire, read back through the
//! `DescribeInputDevice` and `ListInputDevices` that report it.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the issue's `hd-123456789abcdef`-shaped device, stopped
//! once, is ONE row of them. The `ClaimDevice` / `DescribeInputDevice` helpers
//! are the `input_device_maintenance_window` suite's, so the suites cannot
//! disagree about how a device is claimed or read.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::ops::types::INPUT_DEVICE_STATE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_maintenance_window::{
    claim_device, describe_device, device, start_maintenance_window,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `StopInputDevice`'s binding (`smithy_query(query="operation",
/// operation="StopInputDevice")`: POST /prod/inputDevices/{InputDeviceId}/stop).
async fn stop_device(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/stop"),
        None,
    )
    .await
}

/// `StartInputDevice`'s binding (`smithy_query(query="operation",
/// operation="StartInputDevice")`: POST
/// /prod/inputDevices/{InputDeviceId}/start).
async fn start_device(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/start"),
        None,
    )
    .await
}

/// The one command a row of the state table issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Start,
    Stop,
}

/// Issue `command` against the device `id`, asserting it succeeded with the
/// empty body its response shape models (neither operation models an output
/// member).
async fn command(router: &MultiServiceRouter, case: &str, id: &str, command: Command) {
    let (status, body) = match command {
        Command::Start => start_device(router, id).await,
        Command::Stop => stop_device(router, id).await,
    };
    assert_eq!(
        status, 200,
        "case {case:?}: {command:?}InputDevice({id}): expected 200, got {status}: {body}"
    );
    assert!(
        body.get("message").is_none(),
        "case {case:?}: {command:?}InputDevice({id}): expected a success, got {body}"
    );
}

/// The `DeviceState` the device `id` reports right now, read out of the settings
/// of the device's own type: `hdDeviceSettings` for an HD device,
/// `uhdDeviceSettings` for a UHD one ("Settings that describe an input device
/// that is type HD" / "... type UHD"). The settings of the OTHER type are
/// asserted absent, so a device is described by its own type's settings only.
async fn device_state(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let described = device(router, case, id).await;
    let device_type = described["type"].as_str().unwrap_or_default().to_string();
    let (mine, other) = match device_type.as_str() {
        "HD" => ("hdDeviceSettings", "uhdDeviceSettings"),
        "UHD" => ("uhdDeviceSettings", "hdDeviceSettings"),
        unexpected => panic!("case {case:?}: expected the model's InputDeviceType, got {unexpected:?}"),
    };
    assert!(
        described[other].is_null(),
        "case {case:?}: expected a {device_type} device to carry no {other}, got {:?}",
        described[other]
    );
    let state = described[mine]["deviceState"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a deviceState in {mine}, got {:?}",
                described[mine]
            )
        })
        .to_string();
    // The model's own value list (`smithy_query(query="shape",
    // shape="InputDeviceState")`: IDLE | STREAMING), as the generated types
    // carry it.
    assert!(
        INPUT_DEVICE_STATE_VALUES.contains(&state.as_str()),
        "case {case:?}: expected one of the model's InputDeviceState values \
         {INPUT_DEVICE_STATE_VALUES:?}, got {state:?}"
    );
    state
}

/// One row of the device table: a device id and the `InputDeviceType` its
/// documented prefix names.
struct Row {
    case: &'static str,
    id: &'static str,
    device_type: &'static str,
}

/// The device rows every suite below runs over. They differ along what the rules
/// generalise over: the device's TYPE and the shape of its id (the `hd-`/`uhd-`
/// prefix the `InputDeviceId` member documents by example — "For example,
/// hd-123456789abcdef" —, its case, and the length and character class of the
/// unit identifier after it), because the rules are about the run state of
/// whatever device the id resolves to, not about any one device or id.
const ROWS: [Row; 4] = [
    Row {
        case: "the hd device the operation documents",
        id: "hd-123456789abcdef",
        device_type: "HD",
    },
    Row {
        case: "a uhd device",
        id: "uhd-0a1b2c3d4e5f6071",
        device_type: "UHD",
    },
    Row {
        case: "a uhd device whose label is printed in upper case",
        id: "UHD-A1B2",
        device_type: "UHD",
    },
    Row {
        case: "an hd device with a long identifier",
        id: "hd-00112233445566778899aabbccddeeff",
        device_type: "HD",
    },
];

/// Rule: an input device's `InputDeviceState` is the state the last start or stop
/// accepted for it put it in — `STREAMING` after a start, `IDLE` after a stop and
/// for a device neither has been issued for — and both commands are idempotent.
/// The state is reported as the `DeviceState` of the settings of the device's own
/// type.
///
/// The sequence is exercised over every device row and asserts the state after
/// EVERY command, so what is pinned is the state's dependence on the last
/// command rather than one transition.
#[tokio::test]
async fn a_devices_state_is_the_one_the_last_start_or_stop_left_it_in() {
    for Row {
        case,
        id,
        device_type,
    } in ROWS
    {
        let router = medialive();
        claim_device(&router, case, id).await;

        let claimed = device(&router, case, id).await;
        assert_eq!(
            claimed["type"],
            json!(device_type),
            "case {case:?}: expected the type the id's prefix names, got {:?}",
            claimed["type"]
        );
        // A claimed device has been told to do nothing yet, so it is producing
        // nothing.
        assert_eq!(
            device_state(&router, case, id).await,
            "IDLE",
            "case {case:?}: expected a device nothing has started to be IDLE"
        );

        // (the command, the state it leaves the device in). The repeats are what
        // pin idempotence, and the trailing start is what makes the state the
        // last command's rather than a one-way flag.
        let sequence = [
            (Command::Start, "STREAMING"),
            (Command::Start, "STREAMING"),
            (Command::Stop, "IDLE"),
            (Command::Stop, "IDLE"),
            (Command::Start, "STREAMING"),
            (Command::Stop, "IDLE"),
        ];

        for (issued, expected) in sequence {
            command(&router, case, id, issued).await;
            assert_eq!(
                device_state(&router, case, id).await,
                expected,
                "case {case:?}: expected {issued:?}InputDevice to leave the device {expected}"
            );
        }
    }
}

/// Rule: a start or a stop writes the run state of the device its id names and
/// NOTHING else — no other member of that device, and no other device the account
/// holds.
#[tokio::test]
async fn a_start_or_a_stop_writes_only_the_run_state_of_the_device_it_names() {
    for Row { case, id, .. } in ROWS {
        let router = medialive();
        // A second device nothing is started or stopped, so the suite can tell
        // one device's run state from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;

        // A maintenance window is open on the device, so the suite also pins
        // that the run state and the software status are independent.
        let (status, started) = start_maintenance_window(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, got {status}: {started}"
        );
        let before = device(&router, case, id).await;
        let bystander_before = device(&router, case, bystander).await;

        for issued in [Command::Start, Command::Stop] {
            command(&router, case, id, issued).await;

            let after = device(&router, case, id).await;
            for (member, value) in before.as_object().expect("the describe body is an object") {
                if member == "hdDeviceSettings" || member == "uhdDeviceSettings" {
                    continue;
                }
                assert_eq!(
                    &after[member], value,
                    "case {case:?}: expected {issued:?}InputDevice to leave {member} alone, got {:?}",
                    after[member]
                );
            }
            assert_eq!(
                after["deviceUpdateStatus"],
                json!("UPDATING"),
                "case {case:?}: expected the open maintenance window to be untouched, got {:?}",
                after["deviceUpdateStatus"]
            );
            assert_eq!(
                device(&router, case, bystander).await,
                bystander_before,
                "case {case:?}: expected the device nothing was issued for to be untouched"
            );
        }
    }
}

/// Rule: the run state a device reports is the SAME through every reader — the
/// `DescribeInputDevice` that reports one device and the `ListInputDevices` that
/// reports the store — because both project the one stored device.
#[tokio::test]
async fn both_readers_report_the_same_run_state() {
    let router = medialive();
    for Row { case, id, .. } in ROWS {
        claim_device(&router, case, id).await;
    }
    // Start every other device, so the listing has to report two different
    // states at once.
    for (index, Row { case, id, .. }) in ROWS.iter().enumerate() {
        if index % 2 == 0 {
            command(&router, case, id, Command::Start).await;
        }
    }

    let (status, body) = call(&router, "GET", "/prod/inputDevices", None).await;
    assert_eq!(
        status, 200,
        "ListInputDevices: expected 200, got {status}: {body}"
    );
    let listed = body["inputDevices"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("expected an inputDevices list, got {body}"));
    assert_eq!(
        listed.len(),
        ROWS.len(),
        "expected every claimed device to be listed, got {body}"
    );

    for (index, Row { case, id, .. }) in ROWS.iter().enumerate() {
        let expected = if index % 2 == 0 { "STREAMING" } else { "IDLE" };
        assert_eq!(
            device_state(&router, case, id).await,
            expected,
            "case {case:?}: expected DescribeInputDevice to report {expected}"
        );

        let summary = listed
            .iter()
            .find(|summary| summary["id"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: expected {id} in the listing, got {body}"));
        let described = device(&router, case, id).await;
        for member in ["hdDeviceSettings", "uhdDeviceSettings"] {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the described one, got {:?}",
                summary.get(member)
            );
        }
    }
}

/// Rule: a start or a stop addresses a device this account has CLAIMED, so an id
/// no claim registered is answered with the operation's own modelled
/// `NotFoundException` (404) — carrying this service's not-found wording,
/// resource type then identifier — and nothing is written for it.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn a_start_or_a_stop_of_a_device_the_account_has_not_claimed_is_not_found() {
    let router = medialive();
    // One device exists, so the refusals are about the id and not about an empty
    // store.
    let claimed = "hd-claimed000001";
    claim_device(&router, "claimed", claimed).await;
    command(&router, "claimed", claimed, Command::Start).await;

    for id in [
        "hd-123456789abcdef",
        "uhd-0a1b2c3d",
        "hd-claimed00000",
        "not-a-device",
    ] {
        for (operation, response) in [
            ("StartInputDevice", start_device(&router, id).await),
            ("StopInputDevice", stop_device(&router, id).await),
        ] {
            let (status, body) = response;
            assert_eq!(
                status, 404,
                "case {id:?}: expected {operation} to answer 404 for an unclaimed device, \
                 got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {id:?}: expected {operation}'s modelled NotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("InputDevice {id} not found"),
                "case {id:?}: expected {operation}'s message to name the resource type then \
                 the identifier, got {:?}",
                error_message(&body)
            );

            // The refusal wrote nothing: the id still names no device.
            let (status, described) = describe_device(&router, id).await;
            assert_eq!(
                status, 404,
                "case {id:?}: expected the refused id to still name no device, \
                 got {status}: {described}"
            );
        }
    }

    // The device that does exist kept the state its own start left it in.
    assert_eq!(
        device_state(&router, "claimed", claimed).await,
        "STREAMING",
        "expected the refused commands to leave the claimed device alone"
    );
}
