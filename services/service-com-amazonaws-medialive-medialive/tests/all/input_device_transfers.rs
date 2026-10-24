// SPDX-License-Identifier: CC0-1.0

//! `TransferInputDevice` and `ListInputDeviceTransfers` — the transfer of an
//! input device to another AWS account and the listing that reports the
//! transfers pending on this account's devices, end to end over the restJson1
//! wire, together with the `ClaimDevice` that brings a device into the account.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the issue's single `ListInputDeviceTransfers` call is
//! ONE row of them. The wire harness (`medialive`, `call`, `error_code`) is the
//! crate's shared one in `wire`.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_LIST_PAGE_SIZE, MAX_TRANSFER_MESSAGE_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, medialive, ACCOUNT};

/// The collection URI `ListInputDeviceTransfers` is bound to
/// (`smithy_query(query="operation", operation="ListInputDeviceTransfers")`: GET
/// /prod/inputDeviceTransfers).
const TRANSFERS: &str = "/prod/inputDeviceTransfers";

/// An account id that is not this emulator's own, so a transfer addressed to it
/// is one this account sends OUT and never one it receives. Twelve digits, the
/// form `TargetCustomerId` documents.
const OTHER_ACCOUNT: &str = "123456789012";

/// `ClaimDevice` on one device id, asserting the claim succeeded.
async fn claim_device(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = call(router, "POST", "/prod/claimDevice", Some(json!({ "id": id }))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ClaimDevice({id}): expected 200, got {status}: {body}"
    );
}

/// `TransferInputDevice` on one device with `body` as the request payload.
async fn transfer(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/transfer"),
        Some(body),
    )
    .await
}

/// `ListInputDeviceTransfers` with `query` appended verbatim (every input member
/// is `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{TRANSFERS}{query}"), None).await
}

/// The transfers the listing reports for one direction, asserting the 200 a
/// well-formed listing answers with.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    transfer_type: &str,
) -> Vec<Value> {
    let (status, body) = list(router, &format!("?transferType={transfer_type}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInputDeviceTransfers({transfer_type}): expected 200, got {status}: {body}"
    );
    body["inputDeviceTransfers"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an inputDeviceTransfers list, got {body}")
        })
}

/// The reported transfer of the device `id` in one direction, or `None` when the
/// listing does not report it.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    transfer_type: &str,
    id: &str,
) -> Option<Value> {
    listed(router, case, transfer_type)
        .await
        .into_iter()
        .find(|summary| summary["id"] == json!(id))
}

/// `DescribeInputDevice` on one device.
async fn describe_device(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputDevices/{id}"), None).await
}

/// One row of the transfer table: the device transferred, the request that
/// transfers it, and the members the listing must report back.
struct Row {
    case: &'static str,
    id: &'static str,
    body: Value,
    target_customer_id: Option<String>,
    message: Option<String>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the hd device the operation documents, to another account",
            id: "hd-123456789abcdef",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "targetRegion": "us-east-1",
                "transferMessage": "please take this one",
            }),
            target_customer_id: Some(OTHER_ACCOUNT.to_string()),
            message: Some("please take this one".to_string()),
        },
        Row {
            case: "a uhd device with no message",
            id: "uhd-0a1b2c3d4e5f6071",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "targetRegion": "eu-west-1",
            }),
            target_customer_id: Some(OTHER_ACCOUNT.to_string()),
            message: None,
        },
        Row {
            case: "a device transferred with no recipient and no region",
            id: "hd-norecipient01",
            body: json!({}),
            target_customer_id: None,
            message: None,
        },
        Row {
            // The recipient is THIS account, which is the only incoming
            // transfer a single-account emulator can hold: the same transfer is
            // outgoing for its sender and incoming for its recipient.
            case: "a device transferred to this very account",
            id: "hd-toself0000001",
            body: json!({
                "targetCustomerId": ACCOUNT,
                "transferMessage": "back to you",
            }),
            target_customer_id: Some(ACCOUNT.to_string()),
            message: Some("back to you".to_string()),
        },
        Row {
            // The longest message the member documents ("Maximum 280
            // characters") — the published bound itself.
            case: "a message of exactly the documented maximum length",
            id: "UHD-LONGMESSAGE",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "transferMessage": "m".repeat(MAX_TRANSFER_MESSAGE_CHARS),
            }),
            target_customer_id: Some(OTHER_ACCOUNT.to_string()),
            message: Some("m".repeat(MAX_TRANSFER_MESSAGE_CHARS)),
        },
        Row {
            // The bound is documented in CHARACTERS, so a message of multi-byte
            // characters is measured the way the sender wrote it.
            case: "a message of multi-byte characters at the documented maximum",
            id: "hd-multibyte0001",
            body: json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "transferMessage": "é".repeat(MAX_TRANSFER_MESSAGE_CHARS),
            }),
            target_customer_id: Some(OTHER_ACCOUNT.to_string()),
            message: Some("é".repeat(MAX_TRANSFER_MESSAGE_CHARS)),
        },
    ]
}

/// Rule: `TransferInputDevice` records one pending transfer on the claimed
/// device its id names, and `ListInputDeviceTransfers` reports exactly the
/// devices this account's store holds a pending transfer of in the direction the
/// request asks for — `OUTGOING` for a transfer this account started,
/// `INCOMING` for one whose recipient is this account — each carrying the
/// recipient and the message the transfer recorded, and nothing for a device no
/// transfer is pending on.
///
/// The rows differ along what the rule generalises over: the device type and the
/// shape of its id, whether the request names a recipient, a region or a
/// message, whether the recipient is this account or another one, and the length
/// and character class of the message — because the rule is about the transfer
/// recorded for whatever device the id resolves to, not about any one request.
#[tokio::test]
async fn a_transfer_is_reported_by_the_listing_in_the_direction_it_was_recorded() {
    for Row {
        case,
        id,
        body,
        target_customer_id,
        message,
    } in rows()
    {
        let router = medialive();

        // A second claimed device nothing transfers, so the suite can tell one
        // device's transfer from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;

        // Before the transfer the listing reports neither device, in either
        // direction: the transfers come out of state, not out of the store's
        // device list.
        for direction in ["OUTGOING", "INCOMING"] {
            assert!(
                listed(&router, case, direction).await.is_empty(),
                "case {case:?}: expected no {direction} transfer before one is started"
            );
        }

        let (status, transferred) = transfer(&router, id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {transferred}"
        );

        let outgoing = reported(&router, case, "OUTGOING", id)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the transferred device among the OUTGOING transfers")
            });
        assert_eq!(
            outgoing["transferType"],
            json!("OUTGOING"),
            "case {case:?}: expected the direction the listing was asked for, got {:?}",
            outgoing["transferType"]
        );
        assert_eq!(
            outgoing["targetCustomerId"],
            target_customer_id
                .clone()
                .map_or(Value::Null, Value::String),
            "case {case:?}: expected the recipient the transfer recorded, got {:?}",
            outgoing["targetCustomerId"]
        );
        assert_eq!(
            outgoing["message"],
            message.clone().map_or(Value::Null, Value::String),
            "case {case:?}: expected the message the transfer recorded, got {:?}",
            outgoing["message"]
        );

        // The INCOMING direction reports the transfer exactly when its
        // recipient is this account: "INCOMING for an input device that is
        // being transferred to you".
        let incoming = reported(&router, case, "INCOMING", id).await;
        let is_incoming = target_customer_id.as_deref() == Some(ACCOUNT);
        assert_eq!(
            incoming.is_some(),
            is_incoming,
            "case {case:?}: expected INCOMING to report the transfer only when this account is the recipient, got {incoming:?}"
        );
        if let Some(incoming) = incoming {
            assert_eq!(
                incoming["transferType"],
                json!("INCOMING"),
                "case {case:?}: expected the direction the listing was asked for, got {:?}",
                incoming["transferType"]
            );
            assert_eq!(
                incoming["id"],
                json!(id),
                "case {case:?}: expected the transferred device's own id, got {:?}",
                incoming["id"]
            );
        }

        // The device nothing transferred is reported by neither direction.
        for direction in ["OUTGOING", "INCOMING"] {
            assert!(
                reported(&router, case, direction, bystander).await.is_none(),
                "case {case:?}: expected the untransferred device out of the {direction} listing"
            );
        }

        // The transferred device is still the account's to read: a pending
        // transfer is a transfer, not a delete.
        let (status, described) = describe_device(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the transferred device to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["id"],
            json!(id),
            "case {case:?}: expected the device to answer to the id it was claimed with, got {:?}",
            described["id"]
        );

        // "After you make the request, the other account must accept or reject
        // the transfer": a device already being transferred is waiting on that
        // decision, so a second transfer conflicts instead of replacing it.
        let (status, again) = transfer(
            &router,
            id,
            json!({ "targetCustomerId": OTHER_ACCOUNT, "transferMessage": "a second try" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a second transfer to conflict, got {status}: {again}"
        );
        assert_eq!(
            error_code(status, &again),
            "ConflictException",
            "case {case:?}: expected the operation's modelled ConflictException, got {again}"
        );
        assert_eq!(
            reported(&router, case, "OUTGOING", id).await,
            Some(outgoing),
            "case {case:?}: expected the refused transfer to leave the pending one as it was"
        );
    }
}

/// Rule: a transfer transfers a device this account has CLAIMED, so an id no
/// claim registered is answered with the operation's own modelled
/// `NotFoundException` (404) — carrying this service's not-found wording,
/// resource type then identifier — and no transfer is recorded for it.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn transferring_a_device_the_account_has_not_claimed_is_not_found() {
    let router = medialive();
    // One device exists, so the refusals are about the id and not about an
    // empty store.
    let claimed = "hd-claimed000001";
    claim_device(&router, "claimed", claimed).await;

    for id in [
        "hd-123456789abcdef",
        "uhd-0a1b2c3d",
        "hd-claimed00000",
        "not-a-device",
    ] {
        let (status, body) = transfer(&router, id, json!({ "targetCustomerId": OTHER_ACCOUNT })).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 for an unclaimed device, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {id:?}: expected the operation's modelled NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("InputDevice {id} not found"),
            "case {id:?}: expected the message to name the resource type then the identifier, got {:?}",
            error_message(&body)
        );
    }

    // The refusals recorded nothing: no direction reports a transfer.
    for direction in ["OUTGOING", "INCOMING"] {
        assert!(
            listed(&router, "unclaimed", direction).await.is_empty(),
            "expected a refused transfer to record nothing for {direction}"
        );
    }
}

/// Rule: a transfer request whose recipient is not an AWS account id ("The AWS
/// account ID (12 digits) for the recipient of the device transfer") or whose
/// message is longer than the member's documented maximum ("An optional message
/// for the recipient. Maximum 280 characters.") is refused with the operation's
/// modelled `BadRequestException` (400), and records nothing.
///
/// The rows span each published bound of the account id's digit count and one
/// past it in both directions, a non-digit id of the right length, and the
/// message bound and one character past it — in single-byte and multi-byte
/// characters, because the bound is documented in characters.
#[tokio::test]
async fn a_malformed_transfer_request_is_refused_and_records_nothing() {
    let router = medialive();
    let id = "hd-refusals00001";
    claim_device(&router, "refusals", id).await;

    // (case, the TransferInputDevice body, the expected status)
    let table: Vec<(String, Value, u16)> = vec![
        (
            "an account id one digit short".to_string(),
            json!({ "targetCustomerId": "1".repeat(11) }),
            400,
        ),
        (
            "an account id one digit long".to_string(),
            json!({ "targetCustomerId": "1".repeat(13) }),
            400,
        ),
        (
            "an account id of the right length that is not digits".to_string(),
            json!({ "targetCustomerId": "12345678901x" }),
            400,
        ),
        (
            "an empty account id".to_string(),
            json!({ "targetCustomerId": "" }),
            400,
        ),
        (
            "a message one character past the documented maximum".to_string(),
            json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "transferMessage": "m".repeat(MAX_TRANSFER_MESSAGE_CHARS + 1),
            }),
            400,
        ),
        (
            "a multi-byte message one character past the documented maximum".to_string(),
            json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "transferMessage": "é".repeat(MAX_TRANSFER_MESSAGE_CHARS + 1),
            }),
            400,
        ),
        (
            // The bound itself is accepted, which is what makes the refusals
            // above about the bound and not about long messages in general.
            "a message of exactly the documented maximum".to_string(),
            json!({
                "targetCustomerId": OTHER_ACCOUNT,
                "transferMessage": "m".repeat(MAX_TRANSFER_MESSAGE_CHARS),
            }),
            200,
        ),
    ];

    for (case, body, expected_status) in table {
        let (status, response) = transfer(&router, id, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        if expected_status == 200 {
            assert!(
                reported(&router, &case, "OUTGOING", id).await.is_some(),
                "case {case:?}: expected the accepted transfer to be reported"
            );
        } else {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's modelled BadRequestException, got {response}"
            );
            assert!(
                reported(&router, &case, "OUTGOING", id).await.is_none(),
                "case {case:?}: expected a refused transfer to record nothing"
            );
        }
    }
}

/// Rule: `ListInputDeviceTransfers` takes its direction as a required
/// `__string`, so a value outside the `InputDeviceTransferType` enum (OUTGOING |
/// INCOMING, `smithy_query(query="shape", shape="InputDeviceTransferType")`) is
/// refused with the operation's modelled `BadRequestException` (400), as is a
/// non-positive page size.
///
/// The rows span a value of the right shape that is not in the enum, the
/// enum's own spellings in the wrong case, the empty value, and the page sizes
/// at and past the lower bound of a page size.
#[tokio::test]
async fn a_listing_outside_the_modelled_direction_or_page_size_is_refused() {
    let router = medialive();
    let id = "hd-listrefusal01";
    claim_device(&router, "list refusals", id).await;
    let (status, body) = transfer(&router, id, json!({ "targetCustomerId": OTHER_ACCOUNT })).await;
    assert_eq!(
        status, 200,
        "TransferInputDevice: expected 200, got {status}: {body}"
    );

    for query in [
        "?transferType=INBOUND",
        "?transferType=outgoing",
        "?transferType=Incoming",
        "?transferType=",
        "?transferType=OUTGOING&maxResults=0",
        "?transferType=OUTGOING&maxResults=-3",
    ] {
        let (status, response) = list(&router, query).await;
        assert_eq!(
            status, 400,
            "case {query:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {query:?}: expected the operation's modelled BadRequestException, got {response}"
        );
    }

    // The refusals are about the request: the modelled spellings still answer.
    assert_eq!(
        listed(&router, "after the refusals", "OUTGOING").await.len(),
        1,
        "expected the pending transfer to still be reported"
    );
    // A request that omits the required `transferType` parameter entirely never
    // reaches this operation: the framework's route match needs it, and
    // answers the emulator's own route-level 501 — which is not this
    // operation's rule to assert. The empty VALUE above is the form that does
    // reach the handler.
}

/// Walk every page of the listing, answering the device ids reported in order
/// and the size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    page_size: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = "?transferType=OUTGOING".to_string();
        if let Some(max_results) = page_size {
            query.push_str(&format!("&maxResults={max_results}"));
        }
        if let Some(next) = &token {
            query.push_str(&format!("&nextToken={next}"));
        }

        let (status, body) = list(router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListInputDeviceTransfers to answer 200, got {status}: {body}"
        );
        let page = body["inputDeviceTransfers"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an inputDeviceTransfers list, got {body}")
            });
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} transfers in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for summary in &page {
            ids.push(summary["id"].as_str().unwrap_or_default().to_string());
        }

        token = body["nextToken"].as_str().map(ToString::to_string);
        // A token is handed out only when something is left over, so an empty
        // page can never carry one.
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        if token.is_none() {
            break;
        }
        assert!(
            pages.len() <= 256,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// One row of the pagination table: how many claimed devices are being
/// transferred, how many are claimed but not transferred, and the page size the
/// request asks for.
struct Paging {
    case: &'static str,
    transferring: usize,
    idle: usize,
    page_size: Option<i32>,
}

/// Rule: the listing reports every pending transfer of its direction, each
/// exactly once, in the order of the device ids it holds them under, cut into
/// pages of at most the requested page size bounded by the service's own
/// ceiling, whose token resumes at the first transfer not yet reported and is
/// absent on the last page.
///
/// The rows differ along the COUNT of pending transfers, the count of claimed
/// devices NOT being transferred (which the pages must skip rather than count
/// against the page size) and the page size asked for — none, one, a size that
/// divides the transfers unevenly, no size at all, a size past the ceiling, and
/// more transfers than one default page holds — because that is what the rule
/// generalises over.
#[tokio::test]
async fn the_listing_pages_through_every_pending_transfer_in_device_id_order() {
    let table = [
        Paging {
            case: "nothing transferring",
            transferring: 0,
            idle: 2,
            page_size: None,
        },
        Paging {
            case: "nothing transferring, page of one",
            transferring: 0,
            idle: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one transfer, page of one",
            transferring: 1,
            idle: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three transfers, page of one",
            transferring: 3,
            idle: 2,
            page_size: Some(1),
        },
        Paging {
            case: "five transfers, page of two",
            transferring: 5,
            idle: 3,
            page_size: Some(2),
        },
        Paging {
            case: "four transfers, default page size",
            transferring: 4,
            idle: 0,
            page_size: None,
        },
        Paging {
            case: "three transfers, page size past the ceiling",
            transferring: 3,
            idle: 1,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // More transfers than a single default page holds, so the default
            // page size is itself a cut and not just a ceiling nobody reaches.
            case: "more transfers than one default page holds",
            transferring: MAX_LIST_PAGE_SIZE + 7,
            idle: 0,
            page_size: None,
        },
    ];

    for Paging {
        case,
        transferring,
        idle,
        page_size,
    } in table
    {
        let router = medialive();

        // The ids are minted so that lexicographic order — the order the store
        // holds devices in — is the order they were claimed in, so the expected
        // order is known without reading it back out of the service.
        let mut expected: Vec<String> = Vec::new();
        for index in 0..transferring {
            let id = format!("hd-transfer{index:04}");
            claim_device(&router, case, &id).await;
            let (status, body) =
                transfer(&router, &id, json!({ "targetCustomerId": OTHER_ACCOUNT })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: TransferInputDevice({id}): expected 200, got {status}: {body}"
            );
            expected.push(id);
        }
        for index in 0..idle {
            // Claimed and NOT transferred, and sorting among the transferred
            // ids, so a page that counted them would be caught.
            claim_device(&router, case, &format!("hd-transfeR{index:04}")).await;
        }
        expected.sort();

        let (reported_ids, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            reported_ids, expected,
            "case {case:?}: expected every pending transfer exactly once in device-id order, got pages {pages:?}"
        );

        // Every page but the last is full: the cut is the page size, not an
        // arbitrary split.
        let full = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        for (index, page) in pages.iter().enumerate() {
            if index + 1 < pages.len() {
                assert_eq!(
                    *page, full,
                    "case {case:?}: expected page {index} to be full, got pages {pages:?}"
                );
            }
        }
    }
}
