// SPDX-License-Identifier: CC0-1.0

//! `ClaimDevice` — the claim that brings an AWS Elemental Link device into this
//! account ("After the request succeeds, you will own the device"), end to end
//! over the restJson1 wire, together with the `DescribeInputDevice` and
//! `ListInputDevices` reads that prove what it persisted.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES: one claim of one `hd-`-prefixed device in one region
//! is ONE row of them. The `DescribeInputDevice` reader is the
//! `input_device_maintenance_window` suite's, so the two suites cannot disagree
//! about how a device is read back.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_INPUT_DEVICES;
use smithy_server_runtime::MultiServiceRouter;

use crate::input_device_maintenance_window::{describe_device_in, device_in};
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The URI `ClaimDevice` is bound to
/// (`smithy_query(query="operation", operation="ClaimDevice")`: POST
/// /prod/claimDevice).
const CLAIM_DEVICE: &str = "/prod/claimDevice";

/// The collection URI `ListInputDevices` is bound to
/// (`smithy_query(query="operation", operation="ListInputDevices")`: GET
/// /prod/inputDevices).
const DEVICES: &str = "/prod/inputDevices";

/// `ClaimDevice` on one device id, signed for `region`.
async fn claim(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
) -> (u16, Value) {
    call_in_region(router, region, "POST", CLAIM_DEVICE, Some(json!({ "id": id }))).await
}

/// The device ids this account holds IN `region`, as `ListInputDevices` reports
/// them in one page. The pagination rule is the listing suite's; this suite reads
/// a single page (the store's own bound is below the largest page), to see WHICH
/// devices a claim registered, and where.
async fn claimed_ids_in(router: &MultiServiceRouter, case: &str, region: &str) -> Vec<String> {
    let (status, body) = call_in_region(router, region, "GET", DEVICES, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInputDevices in {region}: expected 200, got {status}: {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: expected every claimed device in one page, got {body}"
    );
    body["inputDevices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an inputDevices list, got {body}"))
        .iter()
        .map(|listed| listed["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the claim table: a device id, the region the claim is signed for,
/// and the `InputDeviceType` the id's documented prefix names.
struct Row {
    case: &'static str,
    region: &'static str,
    id: &'static str,
    device_type: &'static str,
}

/// Rule: `ClaimDevice` registers, in the region the request was signed for, the
/// device its `Id` names and nothing else — the id is the unit's own, so it is
/// persisted exactly as the request spelled it, the device's `Type` is the one
/// its documented id prefix names, its ARN names that region, and the response
/// itself carries no members (`ClaimDeviceResponse` models none). The device
/// belongs to that region alone: no other region's endpoint reads it or lists
/// it.
///
/// The rows differ along what the rule generalises over: both
/// `InputDeviceType`s, the case the id prefix is spelled in, the length and
/// character class of the unit identifier after it, and the region the claim is
/// signed for — because the rule is about whatever device id a claim names,
/// wherever it is signed, rather than about any one device, id or region.
#[tokio::test]
async fn a_claim_registers_the_device_its_id_names_in_the_region_it_was_signed_for() {
    // The `InputDeviceType` values are the model's own
    // (`smithy_query(query="shape", shape="InputDeviceType")`: HD | UHD), and
    // every one of them is exercised.
    let table: [Row; 4] = [
        Row {
            case: "the hd device the id member documents",
            region: DEFAULT_REGION,
            id: "hd-123456789abcdef",
            device_type: "HD",
        },
        Row {
            case: "a uhd device claimed in another region",
            region: "eu-west-1",
            id: "uhd-0a1b2c3d4e5f6071",
            device_type: "UHD",
        },
        Row {
            case: "an hd device whose label is printed in upper case",
            region: "ap-northeast-1",
            id: "HD-A1B2",
            device_type: "HD",
        },
        Row {
            case: "a uhd device with a long identifier",
            region: "us-east-1",
            id: "uhd-00112233445566778899aabbccddeeff",
            device_type: "UHD",
        },
    ];

    for Row {
        case,
        region,
        id,
        device_type,
    } in table
    {
        let router = medialive();

        // Nothing holds the device before the claim, so what a later read
        // reports is what the claim persisted.
        let (status, before) = describe_device_in(&router, region, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the id to name no device before the claim, got {status}: {before}"
        );

        let (status, claimed) = claim(&router, region, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ClaimDevice({id}): expected 200, got {status}: {claimed}"
        );
        assert!(
            claimed.as_object().is_some_and(|members| members.is_empty()) || claimed.is_null(),
            "case {case:?}: expected a response with no members, got {claimed}"
        );

        let stored = device_in(&router, case, region, id).await;
        assert_eq!(
            stored["id"],
            json!(id),
            "case {case:?}: expected the device to be stored under the id the claim spelled, got {:?}",
            stored["id"]
        );
        assert_eq!(
            stored["type"],
            json!(device_type),
            "case {case:?}: expected the InputDeviceType the id prefix names, got {:?}",
            stored["type"]
        );
        assert_eq!(
            stored["arn"],
            json!(format!("arn:aws:medialive:{region}:{ACCOUNT}:inputDevice:{id}")),
            "case {case:?}: expected an ARN in the region the claim was signed for, got {:?}",
            stored["arn"]
        );

        assert_eq!(
            claimed_ids_in(&router, case, region).await,
            vec![id.to_string()],
            "case {case:?}: expected the claim to register exactly the device it named"
        );

        // The claim registered the device in ITS region, so another region's
        // endpoint neither reads it nor lists it.
        let elsewhere = other_region(region);
        let (status, described) = describe_device_in(&router, elsewhere, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {elsewhere} to hold no device {id}, got {status}: {described}"
        );
        assert_eq!(
            claimed_ids_in(&router, case, elsewhere).await,
            Vec::<String>::new(),
            "case {case:?}: expected {elsewhere} to list no device of {region}"
        );
    }
}

/// A region that is not `region`, for an assertion about what one region's
/// endpoint cannot see of another's.
fn other_region(region: &str) -> &'static str {
    if region == "eu-west-3" {
        "ca-central-1"
    } else {
        "eu-west-3"
    }
}

/// One row of the repeated-claim table: the device claimed first, and the region
/// the second claim of it is signed for.
struct Repeat {
    case: &'static str,
    id: &'static str,
    first_region: &'static str,
    second_region: &'static str,
}

/// Rule: a claim of a device the account already owns IN THE REGION the claim is
/// signed for is IDEMPOTENT — the promise that you own the device already holds
/// there, so the second claim succeeds and leaves the stored device exactly as it
/// is (ARN included, so the region the first claim recorded is not re-written)
/// and registers no second device in that region. A claim signed for ANOTHER
/// region registers the device in that region instead, under that region's ARN,
/// and leaves the first region's device exactly as it was: a regional endpoint
/// answers out of its own region's resources and has no view of another's.
///
/// The rows differ in the device's type and in whether the repeat is signed for
/// the same region as the first claim, because the rule is about a device the
/// account already owns rather than about any one claim pair.
#[tokio::test]
async fn a_repeated_claim_leaves_the_device_the_account_already_owns_as_it_is() {
    let table: [Repeat; 3] = [
        Repeat {
            case: "the same region twice",
            id: "hd-abcdef0123456789",
            first_region: DEFAULT_REGION,
            second_region: DEFAULT_REGION,
        },
        Repeat {
            case: "a uhd device re-claimed in another region",
            id: "uhd-abcdef0123456789",
            first_region: DEFAULT_REGION,
            second_region: "eu-central-1",
        },
        Repeat {
            case: "an hd device first claimed outside the default region",
            id: "hd-0f1e2d3c",
            first_region: "sa-east-1",
            second_region: "us-east-2",
        },
    ];

    for Repeat {
        case,
        id,
        first_region,
        second_region,
    } in table
    {
        let router = medialive();

        let (status, claimed) = claim(&router, first_region, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ClaimDevice({id}): expected 200, got {status}: {claimed}"
        );
        let stored = device_in(&router, case, first_region, id).await;

        let (status, repeated) = claim(&router, second_region, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated claim to succeed, got {status}: {repeated}"
        );
        assert_eq!(
            device_in(&router, case, first_region, id).await,
            stored,
            "case {case:?}: expected the repeated claim to change nothing about the device the first region holds"
        );
        assert_eq!(
            claimed_ids_in(&router, case, first_region).await,
            vec![id.to_string()],
            "case {case:?}: expected the repeated claim to register no second device"
        );
        // The second claim's own region holds the device it claimed, under an
        // ARN naming that region — the same device id in the region whose
        // endpoint took the claim.
        let second = device_in(&router, case, second_region, id).await;
        assert_eq!(
            second["arn"],
            json!(format!(
                "arn:aws:medialive:{second_region}:{ACCOUNT}:inputDevice:{id}"
            )),
            "case {case:?}: expected the device the second region holds to carry its own region's ARN, got {:?}",
            second["arn"]
        );
        assert_eq!(
            claimed_ids_in(&router, case, second_region).await,
            vec![id.to_string()],
            "case {case:?}: expected the second region to hold exactly the device it claimed"
        );
    }
}

/// Rule: the device store is BOUNDED, so once it holds as many devices as it
/// keeps a claim of a device that is not already in it is refused with the
/// operation's own modelled `TooManyRequestsException` (HTTP 429) and registers
/// nothing, while a claim of a device the account already owns still succeeds —
/// it needs no room.
#[tokio::test]
async fn a_claim_past_the_stores_bound_is_refused_and_registers_nothing() {
    let router = medialive();
    let case = "a full device store";

    // Both documented id prefixes, so the full store is not one device type's.
    let ids: Vec<String> = (0..MAX_INPUT_DEVICES)
        .map(|index| {
            let prefix = if index % 2 == 0 { "hd" } else { "uhd" };
            format!("{prefix}-full{index:04}")
        })
        .collect();
    for id in &ids {
        let (status, claimed) = claim(&router, DEFAULT_REGION, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ClaimDevice({id}): expected 200 while there is room, got {status}: {claimed}"
        );
    }

    let full = claimed_ids_in(&router, case, DEFAULT_REGION).await;
    assert_eq!(
        full.len(),
        MAX_INPUT_DEVICES,
        "case {case:?}: expected the store to hold {MAX_INPUT_DEVICES} devices, got {}",
        full.len()
    );

    // One more device, of either documented type: no room for it.
    for id in ["hd-overflow0001", "uhd-overflow0001"] {
        let (status, refused) = claim(&router, DEFAULT_REGION, id).await;
        assert_eq!(
            status, 429,
            "case {case:?}: expected a claim past the bound to be refused, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "TooManyRequestsException",
            "case {case:?}: expected the operation's modelled TooManyRequestsException, got {refused}"
        );
        let (status, described) = describe_device_in(&router, DEFAULT_REGION, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused claim to register nothing, got {status}: {described}"
        );
    }

    // A device already owned is still claimable, and still unchanged.
    let owned = &ids[0];
    let stored = device_in(&router, case, DEFAULT_REGION, owned).await;
    let (status, repeated) = claim(&router, DEFAULT_REGION, owned).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a claim of an owned device to succeed with the store full, got {status}: {repeated}"
    );
    assert_eq!(
        device_in(&router, case, DEFAULT_REGION, owned).await,
        stored,
        "case {case:?}: expected the owned device to be left as it is"
    );
    assert_eq!(
        claimed_ids_in(&router, case, DEFAULT_REGION).await,
        full,
        "case {case:?}: expected the refused and repeated claims to leave the store as it was"
    );
}
