// SPDX-License-Identifier: CC0-1.0

//! `ListInputDevices`: which devices the listing reports, in which order, how a
//! page is cut, and that a listed device is the device `DescribeInputDevice`
//! reports — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: one `ListInputDevices` call over a single claimed
//! device — the case the stub issue reproduces — is ONE row of them. The
//! `ClaimDevice` / `DescribeInputDevice` / `StartInputDeviceMaintenanceWindow`
//! helpers are the `input_device_maintenance_window` suite's, so the two suites
//! cannot disagree about how a device is claimed or read.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{MAX_INPUT_DEVICES, MAX_LIST_PAGE_SIZE};
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_maintenance_window::{
    claim_device, device as describe_stored_device, start_maintenance_window,
};
use crate::wire::{call, error_code, medialive};

/// The collection URI `ListInputDevices` is bound to
/// (`smithy_query(query="operation", operation="ListInputDevices")`: GET
/// /prod/inputDevices).
const DEVICES: &str = "/prod/inputDevices";

/// Every member `InputDeviceSummary` and `DescribeInputDeviceResponse` carry, by
/// its wire name (`smithy_query(query="shape", shape="InputDeviceSummary")`).
const MEMBERS: [&str; 16] = [
    "arn",
    "connectionState",
    "deviceSettingsSyncState",
    "deviceUpdateStatus",
    "hdDeviceSettings",
    "id",
    "macAddress",
    "name",
    "networkSettings",
    "serialNumber",
    "type",
    "uhdDeviceSettings",
    "tags",
    "availabilityZone",
    "medialiveInputArns",
    "outputType",
];

/// `ListInputDevices` with `query` appended verbatim (both input members are
/// `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{DEVICES}{query}"), None).await
}

/// The `inputDevices` member as a list, whatever its length
/// (`ListInputDevicesResponse$InputDevices`, jsonName `inputDevices`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["inputDevices"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an inputDevices list, got {body}"))
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
        let mut query = String::new();
        if let Some(max_results) = page_size {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("maxResults={max_results}"));
        }
        if let Some(next) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={next}"));
        }

        let (status, body) = list(router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListInputDevices to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} devices in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for listed_device in &page {
            ids.push(
                listed_device["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
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
            pages.len() <= 64,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// The ids of `devices` claimed devices, spread over both documented id prefixes
/// so a row spans both `InputDeviceType` values, and claimed in an order that is
/// NOT the order the ids sort in (every other device first, then the rest) — so
/// a listing that simply replayed insertion order would be caught.
fn device_ids(devices: usize) -> Vec<String> {
    (0..devices)
        .map(|index| {
            // The id form every input device operation documents by example
            // ("For example, hd-123456789abcdef"); the prefix is what names the
            // device's `InputDeviceType`.
            let prefix = if index % 2 == 0 { "hd" } else { "uhd" };
            format!("{prefix}-list{index:04}")
        })
        .collect()
}

/// One row of the pagination table: how many devices the account has claimed and
/// the page size the request asks for.
struct Paging {
    case: &'static str,
    devices: usize,
    page_size: Option<i32>,
}

/// Rule: a paginated list operation reports every resource of its type this
/// service's store holds, each exactly once, in the order its store keys them
/// by — for an input device, the device's own id, since the id is the Link unit's
/// and not one the service mints — cut into pages of at most the requested page
/// size bounded by the service's own ceiling, whose token resumes at the first
/// resource not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of claimed devices and the page size asked
/// for — none claimed, one, a size that divides the store unevenly, no size at
/// all, a size past the service's ceiling, and more devices than one default
/// page holds — because that is what the rule generalises over. Within each row
/// the devices are claimed out of id order, and span both device types, so the
/// order asserted is the store's and not the claim sequence.
#[tokio::test]
async fn list_input_devices_pages_through_every_claimed_device_in_id_order() {
    let table = [
        Paging {
            case: "no device claimed",
            devices: 0,
            page_size: None,
        },
        Paging {
            case: "no device claimed, page of one",
            devices: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one device, page of one",
            devices: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three devices, page of one",
            devices: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five devices, page of two",
            devices: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four devices, default page size",
            devices: 4,
            page_size: None,
        },
        Paging {
            case: "three devices, page size past the ceiling",
            devices: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        Paging {
            // More devices than a single default page holds, so the default page
            // size is itself a cut and not just a ceiling nobody reaches.
            case: "more devices than one default page holds",
            devices: MAX_LIST_PAGE_SIZE + 7,
            page_size: None,
        },
    ];

    for Paging {
        case,
        devices,
        page_size,
    } in table
    {
        assert!(
            devices <= MAX_INPUT_DEVICES,
            "case {case:?}: the row asks for {devices} devices, past the store bound \
             {MAX_INPUT_DEVICES}"
        );
        let router = medialive();

        let ids = device_ids(devices);
        // Claimed out of id order: the even-indexed ids first, then the odd ones.
        for index in (0..devices).step_by(2).chain((1..devices).step_by(2)) {
            claim_device(&router, case, &ids[index]).await;
        }
        let mut expected = ids.clone();
        expected.sort();

        let (reported, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the listing to report every claimed device once, \
             in device-id order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if devices == 0 {
            1
        } else {
            devices.div_ceil(size)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {devices} devices of {size}, \
             got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected_page = if index + 1 == pages.len() {
                devices - size * index
            } else {
                size
            };
            assert_eq!(
                *page, expected_page,
                "case {case:?}: expected page {index} to carry {expected_page} devices, \
                 got {pages:?}"
            );
        }
    }
}

/// One row of the projection table: the device's id, how many inputs ingest
/// from it, and whether a maintenance window is open on it.
struct Stored {
    case: &'static str,
    id: &'static str,
    inputs: usize,
    in_maintenance_window: bool,
}

/// Rule: a listed resource is the stored resource read through the list shape,
/// so every member it shares with the resource's own describe reports the same
/// value that describe reports for it.
///
/// `InputDeviceSummary` and `DescribeInputDeviceResponse` carry the same sixteen
/// members (`smithy_query(query="shape", shape="InputDeviceSummary")`), so the
/// assertion walks all of them. The rows differ along the device's TYPE and id
/// shape, along how many inputs ingest from it (which is what the derived
/// `OutputType` and `MedialiveInputArns` come out of) and along whether a
/// maintenance window is open (which is what the derived `DeviceUpdateStatus`
/// comes out of), because the derived members are what a second reader could
/// most easily get wrong.
#[tokio::test]
async fn a_listed_device_is_the_device_describe_reports() {
    let table = [
        Stored {
            case: "an hd device nothing ingests from",
            id: "hd-123456789abcdef",
            inputs: 0,
            in_maintenance_window: false,
        },
        Stored {
            case: "a uhd device one input ingests from",
            id: "uhd-0a1b2c3d4e5f6071",
            inputs: 1,
            in_maintenance_window: false,
        },
        Stored {
            case: "an hd device two inputs ingest from",
            id: "hd-00112233445566778899aabbccddeeff",
            inputs: 2,
            in_maintenance_window: false,
        },
        Stored {
            case: "a device inside a maintenance window",
            id: "UHD-A1B2",
            inputs: 0,
            in_maintenance_window: true,
        },
        Stored {
            case: "an attached device inside a maintenance window",
            id: "hd-attached00001",
            inputs: 1,
            in_maintenance_window: true,
        },
    ];

    for Stored {
        case,
        id,
        inputs,
        in_maintenance_window,
    } in table
    {
        let router = medialive();
        claim_device(&router, case, id).await;

        for index in 0..inputs {
            let (status, created) = call(
                &router,
                "POST",
                "/prod/inputs",
                Some(json!({
                    "name": format!("listing-input-{index}"),
                    "type": "INPUT_DEVICE",
                    "inputDevices": [{ "id": id }],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateInput: expected 201, got {status}: {created}"
            );
        }
        if in_maintenance_window {
            let (status, started) = start_maintenance_window(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, \
                 got {status}: {started}"
            );
        }

        let (status, body) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one claimed device, got {body}"
        );
        let summary = &page[0];
        let described = describe_stored_device(&router, case, id).await;

        for member in MEMBERS {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                described.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the one \
                 DescribeInputDevice reports, got {:?} vs {:?}",
                summary.get(member),
                described.get(member)
            );
        }
        assert_eq!(
            summary["id"],
            json!(id),
            "case {case:?}: expected the claimed device's own id, got {:?}",
            summary["id"]
        );
        // The derived members the rows vary over, asserted against the row
        // rather than only against the other reader.
        let expected_output_type = if inputs == 0 { "NONE" } else { "MEDIALIVE_INPUT" };
        assert_eq!(
            summary["outputType"],
            json!(expected_output_type),
            "case {case:?}: expected the listed device to be {expected_output_type}, got {:?}",
            summary["outputType"]
        );
        assert_eq!(
            summary["medialiveInputArns"].as_array().map(Vec::len),
            (inputs > 0).then_some(inputs),
            "case {case:?}: expected {inputs} attached input ARN(s), got {:?}",
            summary["medialiveInputArns"]
        );
        let expected_update_status = if in_maintenance_window {
            "UPDATING"
        } else {
            "UP_TO_DATE"
        };
        assert_eq!(
            summary["deviceUpdateStatus"],
            json!(expected_update_status),
            "case {case:?}: expected the listed device to be {expected_update_status}, got {:?}",
            summary["deviceUpdateStatus"]
        );
    }
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// repeating it over the same store answers the same devices, and the devices
/// themselves read back unchanged.
///
/// The rows differ along how many devices the account holds and how the listing
/// is paged, because a token-resumed walk is where a reporting call would most
/// easily consume state.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    for (case, devices, page_size) in [
        ("one device, unpaged", 1usize, None),
        ("four devices, page of one", 4, Some(1)),
        ("four devices, page of three", 4, Some(3)),
    ] {
        let router = medialive();
        let ids = device_ids(devices);
        for id in &ids {
            claim_device(&router, case, id).await;
        }
        let mut expected = ids.clone();
        expected.sort();

        let before: Vec<Value> = {
            let mut devices_before = Vec::new();
            for id in &expected {
                devices_before.push(describe_stored_device(&router, case, id).await);
            }
            devices_before
        };

        let (first, _) = every_page(&router, case, page_size).await;
        let (second, _) = every_page(&router, case, page_size).await;
        assert_eq!(
            first, expected,
            "case {case:?}: expected the first walk to report every claimed device, got {first:?}"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected a repeated listing to report the same devices, \
             got {second:?} after {first:?}"
        );

        for (id, was) in expected.iter().zip(before) {
            let now = describe_stored_device(&router, case, id).await;
            assert_eq!(
                now, was,
                "case {case:?}: expected the listings to leave {id} exactly as it was, got {now}"
            );
        }
    }
}

/// Rule: a page size that asks for no results at all is not a page size, so the
/// request is refused with this operation's modelled `BadRequestException`
/// (400) — one of the six errors `smithy_query(query="operation",
/// operation="ListInputDevices")` declares for it — and the refusal reports
/// nothing.
///
/// The rows differ along the non-positive size asked for, and the smallest
/// admissible size (1) is listed beside them as the bound that must still
/// succeed.
#[tokio::test]
async fn a_page_size_that_asks_for_nothing_is_refused() {
    let router = medialive();
    claim_device(&router, "a claimed device", "hd-refusal000001").await;

    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {max_results}: expected a non-positive maxResults to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {max_results}: expected BadRequestException, got {body}"
        );
        assert!(
            body.get("inputDevices").is_none(),
            "case {max_results}: expected a refusal to report no devices, got {body}"
        );
    }

    // The bound itself: one result per page is a page size, and succeeds.
    let (status, body) = list(&router, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case 1: expected the smallest admissible page size to succeed, got {status}: {body}"
    );
    assert_eq!(
        listed("case 1", &body).len(),
        1,
        "case 1: expected a page of one, got {body}"
    );
}
