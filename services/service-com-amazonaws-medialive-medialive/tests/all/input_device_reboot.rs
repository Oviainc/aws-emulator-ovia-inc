// SPDX-License-Identifier: CC0-1.0

//! `RebootInputDevice` — the reboot command an input device goes through, end to
//! end over the restJson1 wire, read back through the `DescribeInputDevice` and
//! `ListInputDevices` that report the device.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the `hd-123456789abcdef`-shaped device the operation
//! documents, rebooted once, is ONE row of them. The device rows, the claim, the
//! start and stop commands, the run-state read and the clock wait are the
//! crate's shared ones — reused, not copied.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::ops::types::REBOOT_INPUT_DEVICE_FORCE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_device_maintenance_window::{
    claim_device, describe_device, device, start_maintenance_window,
};
use crate::input_device_streaming::{command, device_state, Command, Row, ROWS};
use crate::wire::{call, error_code, error_message, medialive};

/// `RebootInputDevice`'s binding (`smithy_query(query="operation",
/// operation="RebootInputDevice")`: POST
/// /prod/inputDevices/{InputDeviceId}/reboot), with the request body the row
/// sends — the operation's only other member, `Force`, lives there.
pub(crate) async fn reboot(
    router: &MultiServiceRouter,
    id: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/reboot"),
        body,
    )
    .await
}

/// Send the reboot command `body` to the device `id`, asserting it was accepted
/// with the empty body its response shape models (the operation models no output
/// member).
async fn send_reboot(router: &MultiServiceRouter, case: &str, id: &str, body: Option<Value>) {
    let (status, response) = reboot(router, id, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RebootInputDevice({id}) with {body:?}: expected 200, got {status}: \
         {response}"
    );
    assert!(
        response.get("message").is_none(),
        "case {case:?}: RebootInputDevice({id}): expected a success, got {response}"
    );
}

/// One form of the reboot request: how it spells `Force`, and whether that
/// spelling FORCES the reboot.
///
/// The rows cover every value the model's enum declares
/// (`smithy_query(query="shape", shape="RebootInputDeviceForce")`: NO | YES,
/// asserted against the generated value list below) and the two ways a request
/// can leave the optional member out — no body at all, and a body that carries
/// no `force` — because the rule is about whether the reboot was forced rather
/// than about any one spelling.
struct ForceForm {
    case: &'static str,
    body: Option<Value>,
    forced: bool,
}

fn force_forms() -> Vec<ForceForm> {
    vec![
        ForceForm {
            case: "a forced reboot",
            body: Some(json!({ "force": "YES" })),
            forced: true,
        },
        ForceForm {
            case: "a reboot that asks for no force",
            body: Some(json!({ "force": "NO" })),
            forced: false,
        },
        ForceForm {
            case: "a reboot whose body carries no force member",
            body: Some(json!({})),
            forced: false,
        },
        ForceForm {
            case: "a reboot sent with no body at all",
            body: None,
            forced: false,
        },
    ]
}

/// The run state a row starts from: the commands that establish it and whether
/// the device is producing video when the reboot arrives.
const BASELINES: [(&str, &[Command], bool); 2] = [
    ("a device a start left streaming", &[Command::Start], true),
    ("a device nothing has commanded", &[], false),
];

/// Rule: a reboot command takes the device its id names off the air for as long
/// as the reboot lasts — its `InputDeviceState` is `IDLE` from the moment the
/// call returns until the clock completes the reboot — and only a reboot the
/// request FORCED resumes a stream it interrupted when it completes; after any
/// other reboot the device is left idle.
///
/// The rows differ along what the rule generalises over: the device's TYPE and
/// the shape of its id (the crate's shared device rows), the form the request
/// spells `Force` in (both modelled values and both ways of omitting it) and the
/// run state the device was in when the command arrived.
#[tokio::test]
async fn a_reboot_takes_the_device_off_the_air_and_only_a_forced_one_resumes_the_stream() {
    // The force rows span the model's own value list, as the generated types
    // carry it.
    for value in REBOOT_INPUT_DEVICE_FORCE_VALUES {
        assert!(
            force_forms()
                .iter()
                .any(|form| form.body.as_ref().and_then(|body| body.get("force"))
                    == Some(&json!(value))),
            "expected a row for the modelled RebootInputDeviceForce value {value:?}"
        );
    }

    for ForceForm {
        case: force_case,
        body,
        forced,
    } in force_forms()
    {
        for (baseline, commands, streaming_before) in BASELINES {
            // Every device row is exercised in the same store, so one wait
            // settles the reboot of each of them.
            let router = medialive();
            for Row { case, id, .. } in ROWS {
                let case = format!("{case} / {force_case} / {baseline}");
                claim_device(&router, &case, id).await;
                for issued in commands {
                    command(&router, &case, id, *issued).await;
                }
                assert_eq!(
                    device_state(&router, &case, id).await,
                    if streaming_before { "STREAMING" } else { "IDLE" },
                    "case {case:?}: expected the baseline commands to set the run state"
                );
            }

            for Row { case, id, .. } in ROWS {
                let case = format!("{case} / {force_case} / {baseline}");
                send_reboot(&router, &case, id, body.clone()).await;
                // A rebooting unit is running no video, so the device is off the
                // air the instant the call returns — whatever it was doing
                // before.
                assert_eq!(
                    device_state(&router, &case, id).await,
                    "IDLE",
                    "case {case:?}: expected the rebooting device to be producing nothing"
                );
            }

            wait_for_the_clock();

            let resumed = forced && streaming_before;
            for Row { case, id, .. } in ROWS {
                let case = format!("{case} / {force_case} / {baseline}");
                assert_eq!(
                    device_state(&router, &case, id).await,
                    if resumed { "STREAMING" } else { "IDLE" },
                    "case {case:?}: expected the completed reboot to leave the device {}",
                    if resumed { "streaming again" } else { "idle" }
                );
            }
        }
    }
}

/// Rule: a start or a stop is the device's run state from the moment it is
/// accepted, so a command issued while a reboot is still in flight outlives that
/// reboot: the device ends up where the LAST command left it, not where the
/// reboot would have put it.
#[tokio::test]
async fn a_command_issued_during_a_reboot_outlives_it() {
    // (the command issued during the reboot, the state it leaves the device in
    // once the reboot completes). The stop is the one a resuming reboot could
    // wrongly undo; the start is the one an unforced reboot could.
    let table: [(Command, &str); 2] = [(Command::Stop, "IDLE"), (Command::Start, "STREAMING")];

    for (issued, expected) in table {
        for forced in [true, false] {
            let router = medialive();
            let force_case = if forced { "forced" } else { "unforced" };
            for Row { case, id, .. } in ROWS {
                let case = format!("{case} / {force_case} / {issued:?} during the reboot");
                claim_device(&router, &case, id).await;
                // Streaming first, so a forced reboot owes the device a
                // resumption the command below has to cancel.
                command(&router, &case, id, Command::Start).await;
                send_reboot(
                    &router,
                    &case,
                    id,
                    Some(json!({ "force": if forced { "YES" } else { "NO" } })),
                )
                .await;
                command(&router, &case, id, issued).await;
            }

            wait_for_the_clock();

            for Row { case, id, .. } in ROWS {
                let case = format!("{case} / {force_case} / {issued:?} during the reboot");
                assert_eq!(
                    device_state(&router, &case, id).await,
                    expected,
                    "case {case:?}: expected the device to keep the state its last command set"
                );
            }
        }
    }
}

/// Rule: a reboot writes the run state of the device its id names and NOTHING
/// else — no other member of that device, and no other device the account holds
/// — and every reader reports the one stored device.
#[tokio::test]
async fn a_reboot_writes_only_the_run_state_of_the_device_it_names() {
    for (baseline, commands, _) in BASELINES {
        for Row { case, id, .. } in ROWS {
            let case = format!("{case} / {baseline}");
            let router = medialive();
            // A second device nothing is rebooted on, so the suite can tell one
            // device's reboot from a store-wide change.
            let bystander = "hd-bystander0001";
            claim_device(&router, &case, bystander).await;
            claim_device(&router, &case, id).await;
            for issued in commands {
                command(&router, &case, id, *issued).await;
            }

            // A maintenance window is open on the device, so the suite also pins
            // that the reboot and the software status are independent.
            let (status, started) = start_maintenance_window(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, got {status}: \
                 {started}"
            );
            let before = device(&router, &case, id).await;
            let bystander_before = device(&router, &case, bystander).await;

            send_reboot(&router, &case, id, Some(json!({ "force": "YES" }))).await;

            let after = device(&router, &case, id).await;
            for (member, value) in before.as_object().expect("the describe body is an object") {
                // The settings of the device's own type carry the run state the
                // reboot does move; every other member is held unchanged.
                if member == "hdDeviceSettings" || member == "uhdDeviceSettings" {
                    continue;
                }
                assert_eq!(
                    &after[member], value,
                    "case {case:?}: expected the reboot to leave {member} alone, got {:?}",
                    after[member]
                );
            }
            // No Link unit is connected to this emulator, so a reboot has
            // nothing to reconnect and the maintenance window it did not touch
            // is still open.
            assert_eq!(
                after["connectionState"],
                json!("DISCONNECTED"),
                "case {case:?}: expected the connection state to be untouched, got {:?}",
                after["connectionState"]
            );
            assert_eq!(
                after["deviceUpdateStatus"],
                json!("UPDATING"),
                "case {case:?}: expected the open maintenance window to be untouched, got {:?}",
                after["deviceUpdateStatus"]
            );
            assert_eq!(
                device(&router, &case, bystander).await,
                bystander_before,
                "case {case:?}: expected the device nothing was rebooted on to be untouched"
            );

            // Both readers project the one stored device, so the listing reports
            // the rebooting device exactly as the describe does.
            let (status, body) = call(&router, "GET", "/prod/inputDevices", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListInputDevices: expected 200, got {status}: {body}"
            );
            let summary = body["inputDevices"]
                .as_array()
                .and_then(|listed| listed.iter().find(|summary| summary["id"] == json!(id)))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected {id} in the listing, got {body}"));
            for member in ["hdDeviceSettings", "uhdDeviceSettings"] {
                assert_eq!(
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    after.get(member).cloned().unwrap_or(Value::Null),
                    "case {case:?}: expected the listed {member} to equal the described one, \
                     got {:?}",
                    summary.get(member)
                );
            }
        }
    }
}

/// Rule: a reboot addresses a device this account has CLAIMED, so an id no claim
/// registered is answered with the operation's own modelled `NotFoundException`
/// (404) — carrying this service's not-found wording, resource type then
/// identifier — and nothing is written for it.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn a_reboot_of_a_device_the_account_has_not_claimed_is_not_found() {
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
        for body in [Some(json!({ "force": "YES" })), None] {
            let (status, response) = reboot(&router, id, body.clone()).await;
            assert_eq!(
                status, 404,
                "case {id:?}: expected 404 for an unclaimed device, got {status}: {response}"
            );
            assert_eq!(
                error_code(status, &response),
                "NotFoundException",
                "case {id:?}: expected the operation's modelled NotFoundException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                format!("InputDevice {id} not found"),
                "case {id:?}: expected the message to name the resource type then the \
                 identifier, got {:?}",
                error_message(&response)
            );

            // The refusal wrote nothing: the id still names no device.
            let (status, described) = describe_device(&router, id).await;
            assert_eq!(
                status, 404,
                "case {id:?}: expected the refused id to still name no device, got {status}: \
                 {described}"
            );
        }
    }

    // The device that does exist kept the state its own start left it in: no
    // refused reboot reached it.
    assert_eq!(
        device_state(&router, "claimed", claimed).await,
        "STREAMING",
        "expected the refused reboots to leave the claimed device alone"
    );
}
