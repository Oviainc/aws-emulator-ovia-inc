// SPDX-License-Identifier: CC0-1.0

//! `CancelInputDeviceTransfer` — the SENDER's end of an input device transfer,
//! end to end over the restJson1 wire: the `TransferInputDevice` that records
//! one, the `ListInputDeviceTransfers` that reports it while it is pending, and
//! the cancel that withdraws it.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one cancel of one device is ONE row of them. The
//! request helpers are the transfer suite's ([`crate::input_device_transfers`])
//! and the accept suite's ([`crate::input_device_transfer_accept`]), reused
//! rather than copied, on the crate's one wire harness in [`crate::wire`]; what
//! this suite adds is the cancel call itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_transfer_accept::accept;
use crate::input_device_transfers::{
    claim_device, describe_device, listed, reported, transfer, OTHER_ACCOUNT,
};
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// `CancelInputDeviceTransfer` on one device (`smithy_query(query="operation",
/// operation="CancelInputDeviceTransfer")`: POST
/// /prod/inputDevices/{InputDeviceId}/cancel).
async fn cancel(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/cancel"),
        None,
    )
    .await
}

/// The device as `DescribeInputDevice` reports it right now, asserting the 200.
async fn device(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = describe_device(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInputDevice({id}): expected 200, got {status}: {described}"
    );
    described
}

/// The message `CancelInputDeviceTransfer` refuses a device with no pending
/// transfer with.
fn nothing_to_cancel(id: &str) -> String {
    format!("InputDevice {id} is not being transferred, so there is no transfer to cancel")
}

/// One row of the cancelled-transfer table: a device id and the transfer request
/// the cancel withdraws.
struct Row {
    case: &'static str,
    id: &'static str,
    body: Value,
}

/// Rule: `CancelInputDeviceTransfer` WITHDRAWS the transfer the asking account
/// requested on the device its id names — the device stays that account's with
/// no transfer pending on it, so the listing reports it in neither direction,
/// the device can be transferred again, and a second cancel has nothing left to
/// withdraw (the operation's modelled `ConflictException`, 409). The cancel
/// changes nothing else the device reports, and no other device.
///
/// The rows differ along what the rule generalises over: the device's type and
/// the shape of its id (the `hd-`/`uhd-` prefix the `InputDeviceId` member
/// documents by example, and its case), which optional members the transfer
/// carried, and whether its recipient is another account or this very one,
/// because the rule is about withdrawing whatever transfer this account started
/// on the device the id resolves to, not about any one request.
#[tokio::test]
async fn cancelling_a_requested_transfer_withdraws_it_and_keeps_the_device() {
    let table: [Row; 4] = [
        Row {
            case: "the hd device the operation documents, with a region and a message",
            id: "hd-123456789abcdef",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "targetRegion": "us-east-1",
                "transferMessage": "please take this one",
            }),
        },
        Row {
            case: "a uhd device with no optional member beyond the recipient",
            id: "uhd-0a1b2c3d4e5f6071",
            body: json!({ "targetCustomerId": OTHER_ACCOUNT }),
        },
        Row {
            case: "a uhd device whose label is printed in upper case, with no recipient at all",
            id: "UHD-A1B2",
            body: json!({ "targetRegion": "ap-south-1" }),
        },
        Row {
            // The recipient is THIS account, which a single-account emulator
            // reads as incoming as well as outgoing: the sender may still
            // withdraw what it requested.
            case: "a device transferred to this very account",
            id: "hd-toself0000001",
            body: json!({ "targetCustomerId": ACCOUNT, "transferMessage": "back to you" }),
        },
    ];

    for Row { case, id, body } in table {
        let router = medialive();

        // A second claimed device nothing is transferred for, so the suite can
        // tell one device's cancel from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;
        let claimed = device(&router, case, id).await;

        let (status, started) = transfer(&router, id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
        );
        assert!(
            reported(&router, case, "OUTGOING", id).await.is_some(),
            "case {case:?}: expected the started transfer among the OUTGOING transfers"
        );

        let (status, cancelled) = cancel(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CancelInputDeviceTransfer({id}): expected 200, got {status}: \
             {cancelled}"
        );
        assert_eq!(
            cancelled,
            json!({}),
            "case {case:?}: expected the modelled empty response, got {cancelled}"
        );

        // The transfer is withdrawn: the listing reports it in NEITHER
        // direction, because no transfer is pending on the device any more.
        for direction in ["INCOMING", "OUTGOING"] {
            assert!(
                listed(&router, case, direction).await.is_empty(),
                "case {case:?}: expected the cancelled transfer out of the {direction} listing"
            );
        }

        // The device never left this account, so it reads back exactly as it did
        // before the transfer was ever requested.
        assert_eq!(
            device(&router, case, id).await,
            claimed,
            "case {case:?}: expected the cancelled device to read back unchanged"
        );

        // Nothing is left to cancel...
        let (status, twice) = cancel(&router, id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a second cancel to conflict, got {status}: {twice}"
        );
        assert_eq!(
            error_code(status, &twice),
            "ConflictException",
            "case {case:?}: expected the operation's modelled ConflictException, got {twice}"
        );
        assert_eq!(
            error_message(&twice),
            nothing_to_cancel(id),
            "case {case:?}: expected the refusal to say the device is not being transferred, got \
             {:?}",
            error_message(&twice)
        );

        // ... and the device can be transferred on again, which a device with a
        // transfer still pending could not be.
        let (status, onwards) = transfer(&router, id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a cancelled device to be transferable again, got {status}: \
             {onwards}"
        );
        assert!(
            reported(&router, case, "OUTGOING", id).await.is_some(),
            "case {case:?}: expected the new transfer to be pending again"
        );

        // The device nothing was transferred for has no transfer of its own, and
        // the cancel of its neighbour did not give it one.
        let (status, bystanding) = cancel(&router, bystander).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the untouched device to have nothing to cancel, got \
             {status}: {bystanding}"
        );
        assert_eq!(
            error_message(&bystanding),
            nothing_to_cancel(bystander),
            "case {case:?}: expected the untouched device to report no pending transfer, got {:?}",
            error_message(&bystanding)
        );
        assert_eq!(
            device(&router, case, bystander).await["id"],
            json!(bystander),
            "case {case:?}: expected the untouched device to still be claimed"
        );
    }
}

/// Rule: a transfer is pending until exactly ONE decision resolves it, so
/// whichever of cancelling it and accepting it comes first leaves the other with
/// nothing to resolve — the operation's modelled `ConflictException` (409), each
/// refusing in its own words.
///
/// The rows differ in which decision resolves the transfer first and in the
/// recipient the transfer names, because the rule is about a transfer being
/// resolved once rather than about any one way of resolving it.
#[tokio::test]
async fn a_transfer_is_resolved_once_whether_it_is_cancelled_or_accepted() {
    // (case, the transfer request body, whether the accept resolves it first)
    let table: [(&str, Value, bool); 3] = [
        (
            "cancelled first, addressed to another account",
            json!({ "targetCustomerId": OTHER_ACCOUNT }),
            false,
        ),
        (
            "cancelled first, addressed to this account",
            json!({ "targetCustomerId": ACCOUNT, "transferMessage": "yours now" }),
            false,
        ),
        (
            "accepted first, addressed to this account",
            json!({ "targetCustomerId": ACCOUNT }),
            true,
        ),
    ];

    for (case, body, accept_first) in table {
        let router = medialive();
        let id = "hd-resolveonce01";
        claim_device(&router, case, id).await;

        let (status, started) = transfer(&router, id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
        );

        if accept_first {
            let (status, accepted) = accept(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: AcceptInputDeviceTransfer({id}): expected 200, got {status}: \
                 {accepted}"
            );
            let (status, refused) = cancel(&router, id).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected nothing left to cancel, got {status}: {refused}"
            );
            assert_eq!(
                error_message(&refused),
                nothing_to_cancel(id),
                "case {case:?}: expected the cancel to report no pending transfer, got {:?}",
                error_message(&refused)
            );
        } else {
            let (status, cancelled) = cancel(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CancelInputDeviceTransfer({id}): expected 200, got {status}: \
                 {cancelled}"
            );
            let (status, refused) = accept(&router, id).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected nothing left to accept, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: expected the modelled ConflictException, got {refused}"
            );
        }

        // Either way the transfer is over: the listing reports it in neither
        // direction.
        for direction in ["INCOMING", "OUTGOING"] {
            assert!(
                listed(&router, case, direction).await.is_empty(),
                "case {case:?}: expected the resolved transfer out of the {direction} listing"
            );
        }
    }
}

/// Rule: the cancel is ABOUT the device its `InputDeviceId` names, so an id no
/// claim ever registered names no device to withdraw a transfer on and is the
/// operation's modelled `NotFoundException` (404) — never a cancel of nothing.
///
/// The rows differ along what the rule generalises over: the shape of the id (a
/// well-formed `hd-`/`uhd-` id nothing claimed, an id of neither Link form, and
/// an id that only looks like a claimed one), because the rule is about whether
/// the store holds a device for the id rather than about how the id is spelled.
#[tokio::test]
async fn cancelling_a_transfer_for_a_device_no_claim_registered_is_not_found() {
    let table: [(&str, &str); 4] = [
        ("an unclaimed hd id", "hd-ffffffffffffffe"),
        ("an unclaimed uhd id", "uhd-abcdef0123456789"),
        ("an id of neither Link form", "not-a-device-id"),
        ("a prefix of the claimed id", "hd-12345678"),
    ];

    let router = medialive();
    // One claimed device with a transfer pending on it, so each refusal is about
    // the id the request named and not about an empty store.
    let claimed = "hd-123456789abcdef";
    claim_device(&router, "claimed device", claimed).await;
    let (status, started) =
        transfer(&router, claimed, json!({ "targetCustomerId": OTHER_ACCOUNT })).await;
    assert_eq!(
        status, 200,
        "case claimed device: TransferInputDevice({claimed}): expected 200, got {status}: {started}"
    );

    for (case, id) in table {
        let (status, refused) = cancel(&router, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an id naming no claimed device, got {status}: \
             {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "NotFoundException",
            "case {case:?}: expected the operation's modelled NotFoundException, got {refused}"
        );
    }

    // The device that WAS claimed still has its transfer pending: refusing an
    // unknown id withdrew nothing.
    assert!(
        reported(&router, "claimed device", "OUTGOING", claimed)
            .await
            .is_some(),
        "case claimed device: expected the pending transfer to survive the refusals"
    );
}
