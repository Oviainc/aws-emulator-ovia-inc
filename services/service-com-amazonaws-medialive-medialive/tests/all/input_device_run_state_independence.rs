// SPDX-License-Identifier: CC0-1.0

//! The run state `StartInputDevice` writes, held against every OTHER write the
//! account can make on the same input device.
//!
//! `StartInputDevice` is "Start an input device that is attached to a
//! MediaConnect flow", and the whole of what it writes is the device's
//! `InputDeviceState`. This suite pins the other half of that: a device's
//! `InputDeviceState` is written ONLY by a start or a stop, so no other accepted
//! write on the same device — a repeated claim, an update of the members
//! MediaLive records for it, an update of its configurable settings, a
//! maintenance window, a tag write, a tag deletion, a transfer and the accept or
//! cancel that ends one — and no transition the clock owes the device's other
//! transient members moves it.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the device the operation documents by example,
//! streaming, is ONE row of them. The device rows, the start and stop calls, the
//! run-state read, the claim and describe helpers, the ARN form, the transfer and
//! tag request helpers and the clock wait are the crate's shared ones — reused,
//! not copied.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_device_description::device_arn;
use crate::input_device_maintenance_window::{claim_device, device, start_maintenance_window};
use crate::input_device_streaming::{command, device_state, Command, Row, ROWS};
use crate::input_device_transfer_accept::accept;
use crate::input_device_transfer_cancel::cancel;
use crate::input_device_transfers::{reported, transfer};
use crate::resource_tags::{create_tags, delete_tags, expected, listed_tags, tags};
use crate::wire::{call, medialive, ACCOUNT};

/// An account id that is not this emulator's own, so a transfer addressed to it
/// is one this account SENT and the cancel below is the sender withdrawing it.
/// Twelve digits, the form `TargetCustomerId` documents.
const OTHER_ACCOUNT: &str = "210987654321";

/// The tag key this suite writes on a device, and the value it writes with it.
const TAG: (&str, &str) = ("run-state", "independent");

/// One write on an input device that is NOT a start or a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    /// `ClaimDevice` on a device this account already owns.
    Claim,
    /// `UpdateInputDevice` naming the device.
    Rename,
    /// `UpdateInputDevice` configuring the settings of the device's own type.
    Configure,
    /// `StartInputDeviceMaintenanceWindow` on the device.
    MaintenanceWindow,
    /// `CreateTags` on the device's ARN.
    Tag,
    /// `DeleteTags` taking back a tag `CreateTags` put on it.
    Untag,
    /// `TransferInputDevice` to another account.
    Transfer,
    /// `AcceptInputDeviceTransfer` ending a transfer addressed to this account.
    AcceptTransfer,
    /// `CancelInputDeviceTransfer` withdrawing a transfer this account sent.
    CancelTransfer,
}

/// Every writer a row of the table below runs.
const WRITERS: [Writer; 9] = [
    Writer::Claim,
    Writer::Rename,
    Writer::Configure,
    Writer::MaintenanceWindow,
    Writer::Tag,
    Writer::Untag,
    Writer::Transfer,
    Writer::AcceptTransfer,
    Writer::CancelTransfer,
];

/// The commands a row starts from, and the `InputDeviceState` they leave the
/// device in: a device nothing has commanded, one a start left `STREAMING`, and
/// one a stop left `IDLE` again. Each is a baseline a write could only wrongly
/// move away from, and the third is why a baseline is the LAST command's state
/// rather than whether a start ever ran.
const BASELINES: [(&str, &[Command], &str); 3] = [
    ("a device nothing has commanded", &[], "IDLE"),
    ("a device a start left streaming", &[Command::Start], "STREAMING"),
    (
        "a device a stop left idle again",
        &[Command::Start, Command::Stop],
        "IDLE",
    ),
];

/// `UpdateInputDevice` as a client calls it: `PUT /prod/inputDevices/{id}`,
/// asserting the update was accepted.
async fn update(router: &MultiServiceRouter, case: &str, id: &str, body: Value) {
    let (status, updated) = call(
        router,
        "PUT",
        &format!("/prod/inputDevices/{id}"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateInputDevice({id}): expected 200, got {status}: {updated}"
    );
}

/// The settings member that configures THIS device, read from the type the store
/// resolved for it: "The settings that you want to apply to the HD input device"
/// / "... to the UHD input device".
async fn settings_member(router: &MultiServiceRouter, case: &str, id: &str) -> &'static str {
    // The model's own value list (`smithy_query(query="shape",
    // shape="InputDeviceType")`: HD | UHD).
    match device(router, case, id).await["type"].as_str() {
        Some("HD") => "hdDeviceSettings",
        Some("UHD") => "uhdDeviceSettings",
        unexpected => {
            panic!("case {case:?}: expected the model's InputDeviceType, got {unexpected:?}")
        }
    }
}

/// One member of the device as `DescribeInputDevice` reports it right now.
async fn reported_member(router: &MultiServiceRouter, case: &str, id: &str, member: &str) -> Value {
    device(router, case, id).await[member].clone()
}

/// Run `writer` against the device `id` and assert ITS OWN effect landed, so a
/// row that finds the run state unchanged has held it against a write the service
/// really accepted rather than against a call it refused.
async fn apply(router: &MultiServiceRouter, case: &str, id: &str, writer: Writer) {
    match writer {
        // "After the request succeeds, you will own the device" already holds
        // for a device this account owns, so the claim is accepted and changes
        // nothing about it.
        Writer::Claim => claim_device(router, case, id).await,
        Writer::Rename => {
            let name = format!("{id}-named");
            update(router, case, id, json!({ "name": name })).await;
            assert_eq!(
                reported_member(router, case, id, "name").await,
                json!(name),
                "case {case:?}: expected the update to name the device"
            );
        }
        Writer::Configure => {
            let member = settings_member(router, case, id).await;
            update(
                router,
                case,
                id,
                // Values inside the settings' own documented domains
                // (`smithy_query(query="shape",
                // shape="InputDeviceConfigurableSettings")`): an
                // `InputDeviceConfiguredInput` the model declares, and a bitrate
                // and a latency the member states no bound for.
                json!({ member: { "configuredInput": "SDI", "maxBitrate": 20_000_000, "latencyMs": 1_000 } }),
            )
            .await;
            assert_eq!(
                reported_member(router, case, id, "deviceSettingsSyncState").await,
                json!("SYNCING"),
                "case {case:?}: expected the configured device to be synchronising"
            );
        }
        Writer::MaintenanceWindow => {
            let (status, started) = start_maintenance_window(router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartInputDeviceMaintenanceWindow({id}): expected 200, \
                 got {status}: {started}"
            );
            assert_eq!(
                reported_member(router, case, id, "deviceUpdateStatus").await,
                json!("UPDATING"),
                "case {case:?}: expected the open maintenance window to be reported"
            );
        }
        Writer::Tag => {
            tag(router, case, id).await;
            assert_eq!(
                listed_tags(router, case, &device_arn(id)).await,
                expected(&[TAG]),
                "case {case:?}: expected the tag the write put on the device"
            );
        }
        Writer::Untag => {
            tag(router, case, id).await;
            let arn = device_arn(id);
            let (status, untagged) = delete_tags(router, &arn, &[TAG.0]).await;
            assert_eq!(
                status, 204,
                "case {case:?}: DeleteTags({id}): expected 204, got {status}: {untagged}"
            );
            assert_eq!(
                listed_tags(router, case, &arn).await,
                Map::new(),
                "case {case:?}: expected the deleted tag to be gone"
            );
        }
        Writer::Transfer => {
            begin_transfer(router, case, id, OTHER_ACCOUNT).await;
            assert!(
                reported(router, case, "OUTGOING", id).await.is_some(),
                "case {case:?}: expected the pending transfer among the OUTGOING transfers"
            );
        }
        Writer::AcceptTransfer => {
            // A transfer addressed to the account this emulator serves is the
            // one an accept can end.
            begin_transfer(router, case, id, ACCOUNT).await;
            let (status, accepted) = accept(router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: AcceptInputDeviceTransfer({id}): expected 200, got {status}: \
                 {accepted}"
            );
            assert!(
                reported(router, case, "INCOMING", id).await.is_none(),
                "case {case:?}: expected the accepted transfer out of the listing"
            );
        }
        Writer::CancelTransfer => {
            begin_transfer(router, case, id, OTHER_ACCOUNT).await;
            let (status, cancelled) = cancel(router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CancelInputDeviceTransfer({id}): expected 200, got {status}: \
                 {cancelled}"
            );
            assert!(
                reported(router, case, "OUTGOING", id).await.is_none(),
                "case {case:?}: expected the cancelled transfer out of the listing"
            );
        }
    }
}

/// `CreateTags` putting [`TAG`] on the device's ARN, asserting the modelled 204.
async fn tag(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, tagged) = create_tags(
        router,
        &device_arn(id),
        json!({ "tags": tags(&[TAG]) }),
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: CreateTags({id}): expected 204, got {status}: {tagged}"
    );
}

/// `TransferInputDevice` addressing the device `id` to `target`, asserting it was
/// recorded.
async fn begin_transfer(router: &MultiServiceRouter, case: &str, id: &str, target: &str) {
    let (status, started) = transfer(
        router,
        id,
        json!({ "targetCustomerId": target, "targetRegion": "us-east-1" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
    );
}

/// Issue `commands` against the device `id` and assert it then reports `state`.
async fn establish(router: &MultiServiceRouter, case: &str, id: &str, commands: &[Command], state: &str) {
    for issued in commands {
        command(router, case, id, *issued).await;
    }
    assert_eq!(
        device_state(router, case, id).await,
        state,
        "case {case:?}: expected the baseline commands to leave the device {state}"
    );
}

/// Rule: an input device's `InputDeviceState` is written only by the start and
/// stop commands it accepts, so every other write the account makes on the same
/// device leaves it in the state its last command set.
///
/// The rows differ along what the rule generalises over: the device's TYPE and
/// the shape of its id (the crate's shared device rows), the baseline the device
/// is in (never commanded, started, started then stopped) and the WRITE held
/// against it (nine of them, one per other operation that writes a device),
/// because the rule is about which writes own the run state rather than about any
/// one device, state or operation.
#[tokio::test]
async fn no_other_write_on_a_device_moves_its_run_state() {
    for Row { case, id, .. } in ROWS {
        for (baseline, commands, state) in BASELINES {
            for writer in WRITERS {
                let case = format!("{case} / {baseline} / {writer:?}");
                let router = medialive();
                // A second device nothing is written for, so the suite can tell
                // one device's run state from a store-wide change.
                let bystander = "hd-bystander0001";
                claim_device(&router, &case, bystander).await;
                claim_device(&router, &case, id).await;
                establish(&router, &case, id, commands, state).await;

                apply(&router, &case, id, writer).await;

                assert_eq!(
                    device_state(&router, &case, id).await,
                    state,
                    "case {case:?}: expected {writer:?} to leave the device {state}"
                );
                assert_eq!(
                    device_state(&router, &case, bystander).await,
                    "IDLE",
                    "case {case:?}: expected the device nothing was written for to stay IDLE"
                );
            }
        }
    }
}

/// Rule: the transitions the clock owes an input device are the ones its other
/// transient members carry — the maintenance window that closes and the
/// configuration the device takes — and the run state is not one of them, so a
/// device keeps the state its last command set however much time passes.
#[tokio::test]
async fn the_clock_settling_a_devices_other_members_leaves_its_run_state_alone() {
    // The baselines a passing clock could only wrongly move away from: the
    // STREAMING a start left, and the IDLE a stop left after one.
    let baselines: [(&str, &[Command], &str); 2] = [
        ("a device a start left streaming", &[Command::Start], "STREAMING"),
        (
            "a device a stop left idle again",
            &[Command::Start, Command::Stop],
            "IDLE",
        ),
    ];

    for Row { case, id, .. } in ROWS {
        for (baseline, commands, state) in baselines {
            let case = format!("{case} / {baseline}");
            let router = medialive();
            claim_device(&router, &case, id).await;
            establish(&router, &case, id, commands, state).await;

            // Both of the device's transient members are put in flight, so the
            // clock has something of its own to settle.
            apply(&router, &case, id, Writer::MaintenanceWindow).await;
            apply(&router, &case, id, Writer::Configure).await;

            wait_for_the_clock();

            assert_eq!(
                reported_member(&router, &case, id, "deviceUpdateStatus").await,
                json!("UP_TO_DATE"),
                "case {case:?}: expected the clock to have closed the maintenance window"
            );
            assert_eq!(
                reported_member(&router, &case, id, "deviceSettingsSyncState").await,
                json!("SYNCED"),
                "case {case:?}: expected the clock to have synchronised the configuration"
            );
            assert_eq!(
                device_state(&router, &case, id).await,
                state,
                "case {case:?}: expected the device to keep the run state its last command set"
            );
        }
    }
}
