// SPDX-License-Identifier: CC0-1.0

//! `AcceptInputDeviceTransfer` — the end of an input device transfer, end to end
//! over the restJson1 wire: the `TransferInputDevice` that records one, the
//! `ListInputDeviceTransfers` that reports it while it is pending, and the
//! accept that resolves it.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one accept of one device is ONE row of them. The
//! request helpers are the transfer suite's ([`crate::input_device_transfers`]),
//! reused rather than copied, on the crate's one wire harness in
//! [`crate::wire`]; what this suite adds is the accept call itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_transfers::{
    claim_device, describe_device, listed, reported, transfer, OTHER_ACCOUNT,
};
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// `AcceptInputDeviceTransfer` on one device (`smithy_query(query="operation",
/// operation="AcceptInputDeviceTransfer")`: POST
/// /prod/inputDevices/{InputDeviceId}/accept).
pub(crate) async fn accept(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/accept"),
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

/// The message `AcceptInputDeviceTransfer` refuses a device with no pending
/// transfer with.
pub(crate) fn nothing_to_accept(id: &str) -> String {
    format!("InputDevice {id} is not being transferred, so there is no transfer to accept")
}

/// One row of the accepted-transfer table: a device id and the transfer request
/// addressed to the account this emulator serves.
struct Row {
    case: &'static str,
    id: &'static str,
    body: Value,
}

/// Rule: a pending transfer waits on the account it names as its recipient, and
/// `AcceptInputDeviceTransfer` is what ENDS it — the device belongs to that
/// recipient from then on with no transfer pending on it, so the listing reports
/// it in neither direction, the device can be transferred again, and a second
/// accept has nothing left to resolve (the operation's modelled
/// `ConflictException`, 409). The accept changes nothing else the device
/// reports, and no other device.
///
/// The rows differ along what the rule generalises over: the device's type and
/// the shape of its id (the `hd-`/`uhd-` prefix the `InputDeviceId` member
/// documents by example, and its case) and which optional members the transfer
/// carried, because the rule is about ending whatever transfer is pending on the
/// device the id resolves to, not about any one request.
#[tokio::test]
async fn accepting_an_incoming_transfer_ends_it_and_keeps_the_device_in_this_account() {
    let table: [Row; 3] = [
        Row {
            case: "the hd device the operation documents, with a region and a message",
            id: "hd-123456789abcdef",
            body: json!({
                "targetCustomerId": ACCOUNT,
                "targetRegion": "eu-west-1",
                "transferMessage": "take this one over",
            }),
        },
        Row {
            case: "a uhd device with no optional member beyond the recipient",
            id: "uhd-0a1b2c3d4e5f6071",
            body: json!({ "targetCustomerId": ACCOUNT }),
        },
        Row {
            case: "a uhd device whose label is printed in upper case, with a region only",
            id: "UHD-A1B2",
            body: json!({ "targetCustomerId": ACCOUNT, "targetRegion": "ap-south-1" }),
        },
    ];

    for Row { case, id, body } in table {
        let router = medialive();

        // A second claimed device nothing is transferred for, so the suite can
        // tell one device's transfer from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;
        let claimed = device(&router, case, id).await;

        let (status, started) = transfer(&router, id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
        );
        let pending = reported(&router, case, "INCOMING", id)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the transfer addressed to this account among the \
                     INCOMING transfers"
                )
            });
        assert_eq!(
            pending["targetCustomerId"],
            json!(ACCOUNT),
            "case {case:?}: expected the recipient the transfer recorded, got {:?}",
            pending["targetCustomerId"]
        );

        let (status, accepted) = accept(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AcceptInputDeviceTransfer({id}): expected 200, got {status}: \
             {accepted}"
        );
        assert_eq!(
            accepted,
            json!({}),
            "case {case:?}: expected the modelled empty response, got {accepted}"
        );

        // The transfer is over: the listing reports it in NEITHER direction,
        // because there is no longer a transfer pending on the device.
        for direction in ["INCOMING", "OUTGOING"] {
            assert!(
                listed(&router, case, direction).await.is_empty(),
                "case {case:?}: expected the accepted transfer out of the {direction} listing"
            );
        }

        // Ownership is the recipient's, which is the account this emulator
        // serves, so the device reads back exactly as it did.
        assert_eq!(
            device(&router, case, id).await,
            claimed,
            "case {case:?}: expected the accepted device to still be this account's, unchanged"
        );

        // Nothing is left to accept...
        let (status, twice) = accept(&router, id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a second accept to conflict, got {status}: {twice}"
        );
        assert_eq!(
            error_code(status, &twice),
            "ConflictException",
            "case {case:?}: expected the operation's modelled ConflictException, got {twice}"
        );
        assert_eq!(
            error_message(&twice),
            nothing_to_accept(id),
            "case {case:?}: expected the refusal to say the device is not being transferred, got \
             {:?}",
            error_message(&twice)
        );

        // ... and the device can be transferred on again, which a device with a
        // transfer still pending could not be.
        let (status, onwards) = transfer(&router, id, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an accepted device to be transferable again, got {status}: \
             {onwards}"
        );
        assert!(
            reported(&router, case, "INCOMING", id).await.is_some(),
            "case {case:?}: expected the new transfer to be pending again"
        );

        // The device nothing was transferred for has no transfer of its own.
        let (status, bystanding) = accept(&router, bystander).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the untouched device to have nothing to accept, got \
             {status}: {bystanding}"
        );
        assert_eq!(
            error_message(&bystanding),
            nothing_to_accept(bystander),
            "case {case:?}: expected the untouched device to report no pending transfer, got {:?}",
            error_message(&bystanding)
        );
    }
}

/// Rule: only the RECIPIENT of a transfer can accept it — the model's two
/// directions are relative to the account asking ("INCOMING for an input device
/// that is being transferred to you, OUTGOING for an input device that you are
/// transferring to another AWS account") — so an account that accepts a transfer
/// addressed to a different account is refused with the operation's modelled
/// `ConflictException` (409), and the transfer stays pending exactly as it was.
///
/// The rows differ in the recipient the transfer names: another account id, a
/// second one made of different digits, and no recipient at all (the member is
/// optional in the model), because the rule is about whether the account asking
/// is the recipient rather than about any one account id.
#[tokio::test]
async fn a_transfer_addressed_to_another_account_cannot_be_accepted_by_this_one() {
    // (case, the transfer request body, the recipient the refusal names)
    let table: [(&str, Value, String); 3] = [
        (
            "another account",
            json!({ "targetCustomerId": OTHER_ACCOUNT }),
            format!("account {OTHER_ACCOUNT}"),
        ),
        (
            "another account, with a message",
            json!({ "targetCustomerId": "999999999999", "transferMessage": "yours now" }),
            "account 999999999999".to_string(),
        ),
        (
            "no recipient at all",
            json!({ "targetRegion": "us-east-1" }),
            "no account".to_string(),
        ),
    ];

    for (case, body, recipient) in table {
        let router = medialive();
        let id = "hd-outgoing00001";
        claim_device(&router, case, id).await;

        let (status, started) = transfer(&router, id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {started}"
        );
        let outgoing = reported(&router, case, "OUTGOING", id).await;
        assert!(
            outgoing.is_some(),
            "case {case:?}: expected the started transfer among the OUTGOING transfers"
        );

        // Refused twice, identically: the first refusal wrote nothing.
        for attempt in 1..=2 {
            let (status, refused) = accept(&router, id).await;
            assert_eq!(
                status, 409,
                "case {case:?}: accept #{attempt}: expected 409 for a transfer this account is \
                 not the recipient of, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: accept #{attempt}: expected the modelled ConflictException, got \
                 {refused}"
            );
            assert_eq!(
                error_message(&refused),
                format!(
                    "InputDevice {id} is being transferred to {recipient}, and only the recipient \
                     of a transfer can accept it"
                ),
                "case {case:?}: accept #{attempt}: expected the refusal to name the recipient the \
                 transfer addresses, got {:?}",
                error_message(&refused)
            );
        }

        // The refusals left the transfer exactly as it was: the listing still
        // reports the same pending transfer.
        assert_eq!(
            reported(&router, case, "OUTGOING", id).await,
            outgoing,
            "case {case:?}: expected the refused accepts to leave the pending transfer alone"
        );
        assert!(
            reported(&router, case, "INCOMING", id).await.is_none(),
            "case {case:?}: expected a transfer addressed elsewhere to be incoming to nobody here"
        );
    }
}

/// Rule: the accept is ABOUT the device its `InputDeviceId` names, so an id no
/// claim ever registered names no device to resolve a transfer on and is the
/// operation's modelled `NotFoundException` (404) — never an accept of nothing.
///
/// The rows differ along what the rule generalises over: the shape of the id (a
/// well-formed `hd-`/`uhd-` id nothing claimed, an id of neither Link form, and
/// an id that only looks like a claimed one), because the rule is about whether
/// the store holds a device for the id rather than about how the id is spelled.
#[tokio::test]
async fn accepting_a_transfer_for_a_device_no_claim_registered_is_not_found() {
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
    let (status, started) = transfer(&router, claimed, json!({ "targetCustomerId": ACCOUNT })).await;
    assert_eq!(
        status, 200,
        "case claimed device: TransferInputDevice({claimed}): expected 200, got {status}: {started}"
    );

    for (case, id) in table {
        let (status, refused) = accept(&router, id).await;
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
    // unknown id resolved nothing.
    assert!(
        reported(&router, "claimed device", "INCOMING", claimed)
            .await
            .is_some(),
        "case claimed device: expected the pending transfer to survive the refusals"
    );
}
