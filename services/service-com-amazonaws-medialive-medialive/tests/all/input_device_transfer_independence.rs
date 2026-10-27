// SPDX-License-Identifier: CC0-1.0

//! The pending transfer `TransferInputDevice` records, held against every OTHER
//! write the account can make on the same input device and against the passing
//! of time.
//!
//! `TransferInputDevice` is "Start an input device transfer to another AWS
//! account. After you make the request, the other account must accept or reject
//! the transfer", so the whole of what it writes is ONE pending transfer on the
//! device its id names, and that transfer waits on the recipient's decision —
//! not on the clock and not on anything else the owner does to the device. This
//! suite pins both halves: the transfer changes no other member the device
//! reports, and no other accepted write on the same device (a repeated claim, a
//! rename, an update of its configurable settings, a maintenance window, a tag
//! write, a tag deletion, a start, a stop, a reboot) and no amount of elapsed
//! time moves or ends it.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one transfer of one device is ONE row of them. The
//! claim, describe, transfer, listing, maintenance-window, update, command,
//! reboot and tag request helpers are the crate's shared ones — reused, not
//! copied.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_device_description::device_arn;
use crate::input_device_maintenance_window::{device, start_maintenance_window};
use crate::input_device_reboot::reboot;
use crate::input_device_run_state_independence::{settings_member, update};
use crate::input_device_streaming::{command, device_state, Command};
use crate::input_device_transfers::{claim_device, reported, transfer, OTHER_ACCOUNT};
use crate::resource_tags::{create_tags, delete_tags, expected, listed_tags, tags};
use crate::wire::{error_code, medialive, ACCOUNT};

/// The tag key this suite writes on a device, and the value it writes with it.
const TAG: (&str, &str) = ("pending-transfer", "untouched");

/// One write on an input device that neither starts a transfer nor ends one.
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
    /// `StartInputDevice` putting the device on the air.
    Start,
    /// `StopInputDevice` taking it off again.
    Stop,
    /// `RebootInputDevice` on a device that was streaming.
    Reboot,
}

/// Every writer a row of the table below holds a pending transfer against.
const WRITERS: [Writer; 9] = [
    Writer::Claim,
    Writer::Rename,
    Writer::Configure,
    Writer::MaintenanceWindow,
    Writer::Tag,
    Writer::Untag,
    Writer::Start,
    Writer::Stop,
    Writer::Reboot,
];

/// One row of the table: the device transferred and the transfer request that
/// records the transfer pending on it.
struct Row {
    case: &'static str,
    id: &'static str,
    body: Value,
}

/// The rows every suite below runs over. They differ along what the rule
/// generalises over: the device's TYPE and the shape of its id (the `hd-`/`uhd-`
/// prefix the `InputDeviceId` member documents by example — "For example,
/// hd-123456789abcdef" — and its case), and which optional members the transfer
/// carries, including the recipient that decides whether the pending transfer is
/// incoming to this account as well as outgoing from it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the hd device the operation documents, outgoing to another account",
            id: "hd-123456789abcdef",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "targetRegion": "us-east-1",
                "transferMessage": "please take this one",
            }),
        },
        Row {
            case: "a uhd device transferred to this very account",
            id: "uhd-0a1b2c3d4e5f6071",
            body: json!({ "targetCustomerId": ACCOUNT, "transferMessage": "back to you" }),
        },
        Row {
            case: "a uhd device whose label is printed in upper case, with no optional member",
            id: "UHD-A1B2",
            body: json!({}),
        },
    ]
}

/// The transfer pending on the device `id` as the listing reports it in each
/// direction: `(OUTGOING, INCOMING)`, each `None` when that direction reports no
/// transfer of the device.
async fn pending(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
) -> (Option<Value>, Option<Value>) {
    (
        reported(router, case, "OUTGOING", id).await,
        reported(router, case, "INCOMING", id).await,
    )
}

/// `CreateTags` putting [`TAG`] on the device's ARN, asserting the modelled 204.
async fn tag(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, tagged) = create_tags(router, &device_arn(id), json!({ "tags": tags(&[TAG]) })).await;
    assert_eq!(
        status, 204,
        "case {case:?}: CreateTags({id}): expected 204, got {status}: {tagged}"
    );
}

/// Run `writer` against the device `id` and assert ITS OWN effect landed, so a
/// row that finds the pending transfer unchanged has held it against a write the
/// service really accepted rather than against a call it refused.
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
                device(router, case, id).await["name"],
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
                device(router, case, id).await["deviceSettingsSyncState"],
                json!("SYNCING"),
                "case {case:?}: expected the configured device to be synchronising"
            );
        }
        Writer::MaintenanceWindow => {
            let (status, started) = start_maintenance_window(router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartInputDeviceMaintenanceWindow({id}): expected 200, got \
                 {status}: {started}"
            );
            assert_eq!(
                device(router, case, id).await["deviceUpdateStatus"],
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
        Writer::Start => {
            command(router, case, id, Command::Start).await;
            assert_eq!(
                device_state(router, case, id).await,
                "STREAMING",
                "case {case:?}: expected the start to put the device on the air"
            );
        }
        Writer::Stop => {
            command(router, case, id, Command::Start).await;
            command(router, case, id, Command::Stop).await;
            assert_eq!(
                device_state(router, case, id).await,
                "IDLE",
                "case {case:?}: expected the stop to take the device off the air"
            );
        }
        Writer::Reboot => {
            command(router, case, id, Command::Start).await;
            let (status, rebooted) = reboot(router, id, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: RebootInputDevice({id}): expected 200, got {status}: {rebooted}"
            );
            assert_eq!(
                device_state(router, case, id).await,
                "IDLE",
                "case {case:?}: expected the reboot to take the streaming device off the air"
            );
        }
    }
}

/// Rule: the whole of what starting an input device transfer writes is ONE
/// pending transfer on the device its id names — it changes no other member that
/// device reports and no other device — and that pending transfer is ended only
/// by the recipient's decision or the sender's cancel, so every other accepted
/// write on the same device leaves it exactly as the transfer recorded it, in
/// each direction the listing reads it and in the conflict a second transfer of a
/// device already being transferred is refused with.
///
/// The rows differ along what the rule generalises over: the device's type and
/// the shape of its id, which optional members the transfer carried and whether
/// its recipient is this account, and the WRITE held against it (nine of them,
/// one per other operation that writes a device), because the rule is about which
/// calls own a pending transfer rather than about any one device or request.
#[tokio::test]
async fn no_other_write_on_a_device_touches_its_pending_transfer() {
    for Row { case, id, body } in rows() {
        for writer in WRITERS {
            let case = format!("{case} / {writer:?}");
            let router = medialive();

            // A second claimed device nothing is transferred for, so the suite
            // can tell one device's transfer from a store-wide change.
            let bystander = "hd-bystander0001";
            claim_device(&router, &case, bystander).await;
            claim_device(&router, &case, id).await;

            let claimed = device(&router, &case, id).await;
            let (status, started) = transfer(&router, id, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
            );
            // The transfer is the ONLY thing the call wrote: the device reads
            // back exactly as the claim left it.
            assert_eq!(
                device(&router, &case, id).await,
                claimed,
                "case {case:?}: expected the transfer to leave every member the device reports \
                 alone"
            );

            let before = pending(&router, &case, id).await;
            assert!(
                before.0.is_some(),
                "case {case:?}: expected the started transfer among the OUTGOING transfers, got \
                 {before:?}"
            );

            apply(&router, &case, id, writer).await;

            assert_eq!(
                pending(&router, &case, id).await,
                before,
                "case {case:?}: expected {writer:?} to leave the pending transfer as the transfer \
                 recorded it"
            );

            // The second reader of the same pending transfer: the device is
            // still waiting on its recipient, so transferring it again
            // conflicts with the transfer already pending on it.
            let (status, again) =
                transfer(&router, id, json!({ "targetCustomerId": OTHER_ACCOUNT })).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected the device to still be being transferred after \
                 {writer:?}, got {status}: {again}"
            );
            assert_eq!(
                error_code(status, &again),
                "ConflictException",
                "case {case:?}: expected the operation's modelled ConflictException, got {again}"
            );

            assert_eq!(
                pending(&router, &case, bystander).await,
                (None, None),
                "case {case:?}: expected the device nothing was transferred for to have no \
                 transfer of its own"
            );
        }
    }
}

/// Rule: a pending input device transfer waits on the RECIPIENT's decision
/// ("[a]fter you make the request, the other account must accept or reject the
/// transfer"), so it is not a transition the clock owes the device: however much
/// time passes, the transfer stays pending exactly as it was recorded, while the
/// device's genuinely transient members — the maintenance window it opens and the
/// configuration it takes — do come due.
///
/// The rows are the same table, so what is held against the clock spans the same
/// device types, id shapes, optional members and recipients.
#[tokio::test]
async fn the_clock_never_ends_a_pending_transfer() {
    for Row { case, id, body } in rows() {
        let router = medialive();
        claim_device(&router, case, id).await;

        let (status, started) = transfer(&router, id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
        );
        let before = pending(&router, case, id).await;

        // Both of the device's transient members are put in flight, so the clock
        // has something of its own to settle.
        apply(&router, case, id, Writer::MaintenanceWindow).await;
        apply(&router, case, id, Writer::Configure).await;

        wait_for_the_clock();

        let settled = device(&router, case, id).await;
        assert_eq!(
            settled["deviceUpdateStatus"],
            json!("UP_TO_DATE"),
            "case {case:?}: expected the clock to have closed the maintenance window, got {:?}",
            settled["deviceUpdateStatus"]
        );
        assert_eq!(
            settled["deviceSettingsSyncState"],
            json!("SYNCED"),
            "case {case:?}: expected the clock to have synchronised the configuration, got {:?}",
            settled["deviceSettingsSyncState"]
        );
        assert_eq!(
            pending(&router, case, id).await,
            before,
            "case {case:?}: expected the transfer to still be waiting on its recipient"
        );
    }
}
