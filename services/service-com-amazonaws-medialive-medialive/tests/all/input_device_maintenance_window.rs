// SPDX-License-Identifier: CC0-1.0

//! `StartInputDeviceMaintenanceWindow` — the maintenance window an input device
//! installs software in, end to end over the restJson1 wire, together with the
//! `ClaimDevice` that brings a device into the account and the
//! `DescribeInputDevice` that reads it back.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the issue's `hd-123456789abcdef`-shaped device is ONE
//! row of each. The wire harness (`medialive`, `call`, `error_code`) is the
//! crate's shared one in `wire`, and the clock wait is the channel suite's —
//! reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, call_in_region, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION};

async fn claim(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/prod/claimDevice", Some(body)).await
}

/// Claim one device by id, asserting the claim succeeded.
pub async fn claim_device(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = claim(router, json!({ "id": id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ClaimDevice({id}): expected 200, got {status}: {body}"
    );
}

/// `DescribeInputDevice` on `id`, signed for `region` — a device belongs to the
/// region its claim was signed for, so a suite whose rule is about that region
/// reads it through this.
pub async fn describe_device_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("/prod/inputDevices/{id}"),
        None,
    )
    .await
}

pub async fn describe_device(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    describe_device_in(router, DEFAULT_REGION, id).await
}

pub async fn start_maintenance_window(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/prod/inputDevices/{id}/startInputDeviceMaintenanceWindow"),
        None,
    )
    .await
}

/// The device as `DescribeInputDevice` reports it right now in `region`.
pub async fn device_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    id: &str,
) -> Value {
    let (status, described) = describe_device_in(router, region, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInputDevice({id}) in {region}: expected 200, got {status}: {described}"
    );
    described
}

/// The device as `DescribeInputDevice` reports it right now.
pub async fn device(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    device_in(router, case, DEFAULT_REGION, id).await
}

/// The `DeviceUpdateStatus` the device reports right now.
async fn update_status(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    device(router, case, id).await["deviceUpdateStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of the maintenance-window table: a device id, and the
/// `InputDeviceType` its documented prefix names.
struct Row {
    case: &'static str,
    id: &'static str,
    device_type: &'static str,
}

/// Rule: `StartInputDeviceMaintenanceWindow` opens a maintenance window on the
/// device its id names — the device's `DeviceUpdateStatus` is `UPDATING` from
/// the moment the call returns until the clock closes the window, and
/// `UP_TO_DATE` again afterwards — and it changes no other device. A start while
/// a window is already open is idempotent, and a closed window can be reopened.
///
/// The rows differ along what the rule generalises over: the device's TYPE and
/// the shape of its id (the `hd-`/`uhd-` prefix the `InputDeviceId` member
/// documents by example — "For example, hd-123456789abcdef" —, its case, and the
/// length and character class of the unit identifier after it), because the rule
/// is about the window a start opens on whatever device the id resolves to, not
/// about any one device or id.
#[tokio::test]
async fn a_maintenance_window_puts_the_device_in_updating_until_the_clock_closes_it() {
    // The `InputDeviceType` values are the model's own
    // (`smithy_query(query="shape", shape="InputDeviceType")`: HD | UHD), and
    // every row of each is exercised.
    let table: [Row; 4] = [
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

    for Row {
        case,
        id,
        device_type,
    } in table
    {
        let router = medialive();

        // A second device nothing starts a window on, so the suite can tell one
        // device's window from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;

        let claimed = device(&router, case, id).await;
        assert_eq!(
            claimed["id"],
            json!(id),
            "case {case:?}: expected the device to be stored under the id it was claimed with, got {:?}",
            claimed["id"]
        );
        assert_eq!(
            claimed["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:inputDevice:{id}"
            )),
            "case {case:?}: expected the ARN built from the request's region and the device id, got {:?}",
            claimed["arn"]
        );
        assert_eq!(
            claimed["type"],
            json!(device_type),
            "case {case:?}: expected the type the id's prefix names, got {:?}",
            claimed["type"]
        );
        assert_eq!(
            claimed["deviceUpdateStatus"],
            json!("UP_TO_DATE"),
            "case {case:?}: expected a claimed device to hold no pending software update, got {:?}",
            claimed["deviceUpdateStatus"]
        );

        // A device id is the unit's own identifier, not a name this service
        // assigns, so it is matched as spelled: the same id in the other case
        // names no device.
        let other_case = if id.chars().any(|letter| letter.is_ascii_uppercase()) {
            id.to_ascii_lowercase()
        } else {
            id.to_ascii_uppercase()
        };
        let (status, body) = describe_device(&router, &other_case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {other_case} to name no device, got {status}: {body}"
        );

        let (status, started) = start_maintenance_window(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, got {status}: {started}"
        );
        assert!(
            started.get("message").is_none(),
            "case {case:?}: expected a success, got {started}"
        );

        // The window is open from the instant the call returns: a caller that
        // reads the device straight afterwards sees the device installing.
        assert_eq!(
            update_status(&router, case, id).await,
            "UPDATING",
            "case {case:?}: expected the device to be UPDATING inside the window"
        );

        // Idempotent: a start while the window is open neither fails nor opens a
        // second one.
        let (status, again) = start_maintenance_window(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated start to succeed, got {status}: {again}"
        );
        assert_eq!(
            update_status(&router, case, id).await,
            "UPDATING",
            "case {case:?}: expected a repeated start to leave the device UPDATING"
        );

        // The window is the only thing that moved: every other member the
        // device reports reads back exactly as it did.
        let inside = device(&router, case, id).await;
        for (member, value) in claimed.as_object().expect("the describe body is an object") {
            if member == "deviceUpdateStatus" {
                continue;
            }
            assert_eq!(
                &inside[member], value,
                "case {case:?}: expected the window to leave {member} alone, got {:?}",
                inside[member]
            );
        }
        assert_eq!(
            update_status(&router, case, bystander).await,
            "UP_TO_DATE",
            "case {case:?}: expected the device nothing started a window on to be untouched"
        );

        wait_for_the_clock();
        assert_eq!(
            update_status(&router, case, id).await,
            "UP_TO_DATE",
            "case {case:?}: expected the closed window to leave the device UP_TO_DATE"
        );

        // A closed window can be reopened, which is what makes the status the
        // window's and not a one-way flag.
        let (status, reopened) = start_maintenance_window(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a later start to succeed, got {status}: {reopened}"
        );
        assert_eq!(
            update_status(&router, case, id).await,
            "UPDATING",
            "case {case:?}: expected the reopened window to put the device back in UPDATING"
        );
    }
}

/// Rule: an input device operation addresses a device this account has CLAIMED,
/// so an id no claim registered is answered with the operation's own modelled
/// `NotFoundException` (404) — carrying this service's not-found wording,
/// resource type then identifier — and nothing is written for it.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn a_window_on_a_device_the_account_has_not_claimed_is_not_found() {
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
        let (status, body) = start_maintenance_window(&router, id).await;
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

        // The refusal wrote nothing: the id still names no device.
        let (status, described) = describe_device(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected the refused id to still name no device, got {status}: {described}"
        );
    }

    assert_eq!(
        update_status(&router, "claimed", claimed).await,
        "UP_TO_DATE",
        "expected the refused starts to leave the claimed device alone"
    );
}

/// Rule: `ClaimDevice` registers the device its `Id` names in the account — the
/// id is the device's own, so it is stored verbatim and a later read answers to
/// it — and refuses, with its modelled `BadRequestException` (400), a request
/// that names no device id or one that is not of the form every input device
/// operation documents. A repeated claim of a device the account already owns is
/// idempotent.
///
/// The rows span both device types, the two malformed-id classes (a prefix with
/// no unit identifier, a string with neither prefix) and the request that omits
/// the member entirely, because the rule is about what a device id IS rather
/// than about any one id.
#[tokio::test]
async fn claiming_a_device_registers_it_and_refuses_what_is_not_a_device_id() {
    let router = medialive();

    // (case, the ClaimDevice body, the expected status)
    let table: [(&str, Value, u16); 7] = [
        ("an hd device", json!({ "id": "hd-abcdef0123456789" }), 200),
        ("a uhd device", json!({ "id": "uhd-abcdef0123456789" }), 200),
        ("an hd prefix with no unit identifier", json!({ "id": "hd-" }), 400),
        ("a uhd prefix with no unit identifier", json!({ "id": "uhd-" }), 400),
        ("an id with neither prefix", json!({ "id": "device-0123456789" }), 400),
        ("an empty id", json!({ "id": "" }), 400),
        ("no id at all", json!({}), 400),
    ];

    for (case, body, expected_status) in table {
        let (status, response) = claim(&router, body.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        let id = body["id"].as_str().unwrap_or_default().to_string();
        if expected_status == 200 {
            // The claim is readable: the device answers to the id it was
            // claimed with, in the state a Link unit nothing has connected is
            // in.
            let claimed = device(&router, case, &id).await;
            assert_eq!(
                claimed["connectionState"],
                json!("DISCONNECTED"),
                "case {case:?}: expected a device with no unit connected to this emulator to be DISCONNECTED, got {:?}",
                claimed["connectionState"]
            );
            assert_eq!(
                claimed["deviceSettingsSyncState"],
                json!("SYNCED"),
                "case {case:?}: expected no pending configuration change, got {:?}",
                claimed["deviceSettingsSyncState"]
            );
            assert_eq!(
                claimed["outputType"],
                json!("NONE"),
                "case {case:?}: expected a device no input ingests from to be attached to nothing, got {:?}",
                claimed["outputType"]
            );
            assert!(
                claimed["medialiveInputArns"].is_null(),
                "case {case:?}: expected no input ARNs while the output type is not MEDIALIVE_INPUT, got {:?}",
                claimed["medialiveInputArns"]
            );
            assert_eq!(
                claimed["tags"],
                json!({}),
                "case {case:?}: expected an untagged device to report the empty tag map, got {:?}",
                claimed["tags"]
            );

            // Idempotent: claiming it again keeps the device as it is.
            let (status, repeated) = claim(&router, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a repeated claim to succeed, got {status}: {repeated}"
            );
            assert_eq!(
                device(&router, case, &id).await,
                claimed,
                "case {case:?}: expected a repeated claim to change nothing"
            );
        } else {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's modelled BadRequestException, got {response}"
            );
            if !id.is_empty() {
                let (status, described) = describe_device(&router, &id).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected a refused claim to register nothing, got {status}: {described}"
                );
            }
        }
    }
}

/// Rule: a device's `OutputType` and `MedialiveInputArns` are DERIVED from the
/// inputs this account holds — `MedialiveInputArns` is "An array of the ARNs for
/// the MediaLive inputs attached to the device", "Returned only if the
/// outputType is MEDIALIVE_INPUT" — so attaching the device to an input, and
/// deleting that input again, is what moves them, and a maintenance window
/// leaves both alone.
///
/// The rows differ in how many inputs ingest from the device, because the rule
/// is about which inputs name the device rather than about any one attachment.
#[tokio::test]
async fn a_devices_attachment_is_derived_from_the_inputs_that_name_it() {
    for inputs in [1usize, 2] {
        let case = format!("{inputs} input(s) naming the device");
        let router = medialive();
        let id = "hd-attached00001";
        claim_device(&router, &case, id).await;

        let mut arns: Vec<String> = Vec::new();
        let mut input_ids: Vec<String> = Vec::new();
        for index in 0..inputs {
            let (status, created) = call(
                &router,
                "POST",
                "/prod/inputs",
                Some(json!({
                    "name": format!("device-input-{index}"),
                    "type": "INPUT_DEVICE",
                    "inputDevices": [{ "id": id }],
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateInput: expected 201, got {status}: {created}"
            );
            // `CreateInput` wraps the input it created in an `Input` member.
            let input = &created["input"];
            arns.push(input["arn"].as_str().unwrap_or_default().to_string());
            input_ids.push(input["id"].as_str().unwrap_or_default().to_string());
        }
        arns.sort();

        let attached = device(&router, &case, id).await;
        assert_eq!(
            attached["outputType"],
            json!("MEDIALIVE_INPUT"),
            "case {case:?}: expected a device an input ingests from to be a MediaLive input source, got {:?}",
            attached["outputType"]
        );
        let mut reported: Vec<String> = attached["medialiveInputArns"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, arns,
            "case {case:?}: expected the ARNs of the inputs that name the device, got {:?}",
            attached["medialiveInputArns"]
        );

        // The window moves the software status and nothing else.
        let (status, started) = start_maintenance_window(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, got {status}: {started}"
        );
        let inside = device(&router, &case, id).await;
        assert_eq!(
            inside["deviceUpdateStatus"],
            json!("UPDATING"),
            "case {case:?}: expected the window to put the attached device in UPDATING, got {:?}",
            inside["deviceUpdateStatus"]
        );
        assert_eq!(
            inside["medialiveInputArns"], attached["medialiveInputArns"],
            "case {case:?}: expected the window to leave the attachment alone, got {:?}",
            inside["medialiveInputArns"]
        );
        assert_eq!(
            inside["outputType"], attached["outputType"],
            "case {case:?}: expected the window to leave the output type alone, got {:?}",
            inside["outputType"]
        );

        // Deleting every input that names the device detaches it again: the two
        // members are derived on every read, never recorded at claim time.
        for input_id in &input_ids {
            let (status, deleted) =
                call(&router, "DELETE", &format!("/prod/inputs/{input_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteInput({input_id}): expected 200, got {status}: {deleted}"
            );
        }
        // Accepting an input's delete releases the device, so an input in the
        // transient `DELETING` step the delete opens is attached no longer: the
        // two members are derived on every read, never recorded at claim time.
        let deleting = device(&router, &case, id).await;
        assert!(
            deleting["medialiveInputArns"].is_null(),
            "case {case:?}: expected an input whose delete was accepted to be detached, got {:?}",
            deleting["medialiveInputArns"]
        );
        assert_eq!(
            deleting["outputType"],
            json!("NONE"),
            "case {case:?}: expected the output type to fall back once the delete was accepted, got {:?}",
            deleting["outputType"]
        );

        wait_for_the_clock();
        let detached = device(&router, &case, id).await;
        assert_eq!(
            detached["outputType"],
            json!("NONE"),
            "case {case:?}: expected a device nothing ingests from any more to be attached to nothing, got {:?}",
            detached["outputType"]
        );
        assert!(
            detached["medialiveInputArns"].is_null(),
            "case {case:?}: expected no input ARNs once the output type is not MEDIALIVE_INPUT, got {:?}",
            detached["medialiveInputArns"]
        );
    }
}
