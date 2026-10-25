// SPDX-License-Identifier: CC0-1.0

//! `DescribeInputDevice` — the one device an id names, read end to end over the
//! restJson1 wire ("Gets the details for the input device").
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the `hd-123456789abcdef`-shaped device the operation
//! documents by example, freshly claimed, is ONE row of them. The rows differ
//! along what the rule generalises over — the device's type and id shape, and
//! which of the account's own calls (a claim, an update, an input that ingests
//! from it, a maintenance window, a start, a tag) wrote the state being read —
//! because the rule is about reporting whatever the store holds for that device.
//!
//! The wire harness (`medialive`, `call`, `error_code`), the claim and describe
//! helpers, the member list and the clock wait are the crate's shared ones —
//! reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_device_listing::MEMBERS;
use crate::input_device_maintenance_window::{
    claim_device, describe_device, device, start_maintenance_window,
};
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION};

/// The ARN a MediaLive input device carries: this service, the region the
/// request was signed for, the account this emulator serves and the device's own
/// id.
fn device_arn(id: &str) -> String {
    format!("arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:inputDevice:{id}")
}

/// What the account did to the device before it is read — each step through the
/// operation a client would use, so every value the read reports was written by
/// the service rather than by the test.
struct Setup {
    /// `UpdateInputDevice`'s body, for a row that names or configures the
    /// device.
    update: Option<Value>,
    /// How many inputs ingest from the device (`CreateInput`), which is what the
    /// derived `OutputType` and `MedialiveInputArns` come out of.
    inputs: usize,
    /// Whether `StartInputDeviceMaintenanceWindow` opened a window on it.
    maintenance_window: bool,
    /// Whether `StartInputDevice` put it in `STREAMING`.
    streaming: bool,
    /// The tags `CreateTags` put on it.
    tags: &'static [(&'static str, &'static str)],
}

/// A device the account has only claimed: the base every row varies from.
fn claimed_only() -> Setup {
    Setup {
        update: None,
        inputs: 0,
        maintenance_window: false,
        streaming: false,
        tags: &[],
    }
}

/// One row of the projection table.
struct Row {
    case: &'static str,
    id: &'static str,
    setup: Setup,
    /// The members the read must report, as a client reads them. Nested objects
    /// are compared member by member ([`assert_reports`]), so a row names only
    /// what it is about.
    expected: Value,
}

/// Run the row's setup against a device, answering the ARNs of the inputs that
/// ingest from it (what `MedialiveInputArns` must report).
async fn apply(router: &MultiServiceRouter, case: &str, id: &str, setup: &Setup) -> Vec<String> {
    claim_device(router, case, id).await;

    if let Some(body) = &setup.update {
        let (status, updated) = call(
            router,
            "PUT",
            &format!("/prod/inputDevices/{id}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInputDevice({id}): expected 200, got {status}: {updated}"
        );
    }

    let mut arns: Vec<String> = Vec::new();
    for index in 0..setup.inputs {
        let (status, created) = call(
            router,
            "POST",
            "/prod/inputs",
            Some(json!({
                "name": format!("describe-input-{index}"),
                "type": "INPUT_DEVICE",
                "inputDevices": [{ "id": id }],
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInput: expected 201, got {status}: {created}"
        );
        arns.push(
            created["input"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    arns.sort();

    if setup.maintenance_window {
        let (status, started) = start_maintenance_window(router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInputDeviceMaintenanceWindow: expected 200, got {status}: {started}"
        );
    }
    if setup.streaming {
        let (status, started) = call(
            router,
            "POST",
            &format!("/prod/inputDevices/{id}/start"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInputDevice({id}): expected 200, got {status}: {started}"
        );
    }
    if !setup.tags.is_empty() {
        let tags: Value = setup
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let (status, tagged) = call(
            router,
            "POST",
            &format!("/prod/tags/{}", percent_encoded(&device_arn(id))),
            Some(json!({ "tags": tags })),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: CreateTags({id}): expected 204, got {status}: {tagged}"
        );
    }

    arns
}

/// Every member of `expected` as the response reports it, walking into nested
/// objects so a row can name one member of a device's settings without
/// restating the rest.
fn assert_reports(case: &str, path: &str, reported: &Value, expected: &Value) {
    if let Value::Object(members) = expected {
        for (member, value) in members {
            let child = reported.get(member).cloned().unwrap_or(Value::Null);
            assert_reports(case, &format!("{path}.{member}"), &child, value);
        }
    } else {
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {path} to be {expected}, got {reported:?}"
        );
    }
}

/// The members every read of every device must agree on, whatever the row wrote:
/// the identity of the device the id resolved to, and the hardware members a
/// unit would supply — no Link unit is connected to this emulator, so they stay
/// absent and the connection state is the `DISCONNECTED` the model gives that
/// situation.
fn assert_identity(case: &str, id: &str, described: &Value) {
    assert_reports(
        case,
        "DescribeInputDevice",
        described,
        &json!({
            "id": id,
            "arn": device_arn(id),
            "connectionState": "DISCONNECTED",
            "macAddress": Value::Null,
            "serialNumber": Value::Null,
            "networkSettings": Value::Null,
        }),
    );

    // The response carries no member the output shape does not declare
    // (`smithy_query(query="operation", operation="DescribeInputDevice")`).
    for member in described.as_object().expect("the describe body is an object").keys() {
        assert!(
            MEMBERS.contains(&member.as_str()),
            "case {case:?}: expected only the members DescribeInputDeviceResponse declares, \
             got {member:?}"
        );
    }

    // A device is described by the settings of its OWN type ("Settings that
    // describe an input device that is type HD" / "... type UHD").
    let device_type = described["type"].as_str().unwrap_or_default();
    let other = match device_type {
        "HD" => "uhdDeviceSettings",
        "UHD" => "hdDeviceSettings",
        unexpected => {
            panic!("case {case:?}: expected the model's InputDeviceType, got {unexpected:?}")
        }
    };
    assert!(
        described[other].is_null(),
        "case {case:?}: expected a {device_type} device to carry no {other}, got {:?}",
        described[other]
    );
}

/// Rule: a describe operation reports the resource its path identifier resolves
/// to as the service's own state holds it — every member either stored on the
/// resource by an earlier accepted call, derived from the rest of the store, or
/// absent because the service has no source for it — so whatever the account's
/// calls wrote is what the read reports back.
///
/// The rows differ along the device's TYPE and id shape and along WHICH call
/// wrote the state read back (a bare claim, an update that names or configures
/// the device, inputs that ingest from it, a maintenance window, a start, a tag
/// request), because the rule is about reporting the stored device rather than
/// about any one member or device.
#[tokio::test]
async fn describe_reports_the_device_the_id_names_out_of_stored_state() {
    let table = [
        Row {
            case: "the hd device the operation documents, only claimed",
            id: "hd-123456789abcdef",
            setup: claimed_only(),
            expected: json!({
                "type": "HD",
                "name": Value::Null,
                "availabilityZone": Value::Null,
                "deviceSettingsSyncState": "SYNCED",
                "deviceUpdateStatus": "UP_TO_DATE",
                "outputType": "NONE",
                "medialiveInputArns": Value::Null,
                "tags": {},
                "hdDeviceSettings": { "deviceState": "IDLE" },
            }),
        },
        Row {
            case: "a uhd device an update named and placed",
            id: "uhd-0a1b2c3d4e5f6071",
            setup: Setup {
                update: Some(json!({ "name": "truck-link", "availabilityZone": "us-west-2b" })),
                ..claimed_only()
            },
            expected: json!({
                "type": "UHD",
                "name": "truck-link",
                "availabilityZone": "us-west-2b",
                // Naming the device changes no DEVICE setting, so nothing is
                // owed to the unit.
                "deviceSettingsSyncState": "SYNCED",
                "uhdDeviceSettings": { "deviceState": "IDLE" },
            }),
        },
        Row {
            case: "a configured hd device with a long identifier",
            id: "hd-00112233445566778899aabbccddeeff",
            setup: Setup {
                update: Some(json!({
                    "hdDeviceSettings": { "configuredInput": "SDI", "maxBitrate": 20_000_000, "latencyMs": 1_000 },
                })),
                ..claimed_only()
            },
            expected: json!({
                "type": "HD",
                "hdDeviceSettings": {
                    "configuredInput": "SDI",
                    "deviceState": "IDLE",
                    "maxBitrate": 20_000_000,
                    "latencyMs": 1_000,
                },
                // The configuration has been sent to the device and not taken
                // yet: "SYNCING means that it has not updated its
                // configuration".
                "deviceSettingsSyncState": "SYNCING",
            }),
        },
        Row {
            case: "a uhd device attached to a mediaconnect flow",
            id: "uhd-flowattached01",
            setup: Setup {
                update: Some(json!({
                    "uhdDeviceSettings": {
                        "codec": "HEVC",
                        "mediaconnectSettings": {
                            "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:link-flow",
                            "sourceName": "link-source",
                        },
                    },
                })),
                ..claimed_only()
            },
            expected: json!({
                "type": "UHD",
                // "Specifies MEDIACONNECT_FLOW if this device is the source for
                // a MediaConnect flow."
                "outputType": "MEDIACONNECT_FLOW",
                "medialiveInputArns": Value::Null,
                "uhdDeviceSettings": {
                    "codec": "HEVC",
                    "deviceState": "IDLE",
                    "mediaconnectSettings": {
                        "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:link-flow",
                        "sourceName": "link-source",
                    },
                },
            }),
        },
        Row {
            case: "an hd device two inputs ingest from",
            id: "hd-attached00001",
            setup: Setup {
                inputs: 2,
                ..claimed_only()
            },
            expected: json!({
                // "Specifies MEDIALIVE_INPUT if this device is the source for a
                // MediaLive input."
                "outputType": "MEDIALIVE_INPUT",
                "deviceUpdateStatus": "UP_TO_DATE",
            }),
        },
        Row {
            case: "a uhd device inside a maintenance window",
            id: "UHD-A1B2",
            setup: Setup {
                maintenance_window: true,
                ..claimed_only()
            },
            expected: json!({
                "type": "UHD",
                // The window is what puts the device's software in UPDATING.
                "deviceUpdateStatus": "UPDATING",
                "uhdDeviceSettings": { "deviceState": "IDLE" },
            }),
        },
        Row {
            case: "a streaming, tagged, attached device",
            id: "hd-streamtagged01",
            setup: Setup {
                update: Some(json!({ "name": "tagged-link" })),
                inputs: 1,
                streaming: true,
                tags: &[("team", "broadcast"), ("env", "prod")],
                ..claimed_only()
            },
            expected: json!({
                "name": "tagged-link",
                "outputType": "MEDIALIVE_INPUT",
                "tags": { "team": "broadcast", "env": "prod" },
                "hdDeviceSettings": { "deviceState": "STREAMING" },
            }),
        },
    ];

    for Row {
        case,
        id,
        setup,
        expected,
    } in table
    {
        let router = medialive();
        // A second device nothing in the row touches, so the suite can tell one
        // device's stored state from a store-wide one.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        let arns = apply(&router, case, id, &setup).await;

        let described = device(&router, case, id).await;
        assert_identity(case, id, &described);
        assert_reports(case, "DescribeInputDevice", &described, &expected);

        // "An array of the ARNs for the MediaLive inputs attached to the
        // device", "Returned only if the outputType is MEDIALIVE_INPUT".
        let mut reported: Vec<String> = described["medialiveInputArns"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        let expected_arns = if described["outputType"] == json!("MEDIALIVE_INPUT") {
            arns.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected_arns,
            "case {case:?}: expected the ARNs of the inputs that ingest from the device, got {:?}",
            described["medialiveInputArns"]
        );

        // The read answered THIS device: the bystander is still the device its
        // own claim left.
        let other = device(&router, case, bystander).await;
        assert_identity(case, bystander, &other);
        assert_reports(
            case,
            "the bystander",
            &other,
            &json!({
                "id": bystander,
                "name": Value::Null,
                "outputType": "NONE",
                "deviceUpdateStatus": "UP_TO_DATE",
                "deviceSettingsSyncState": "SYNCED",
                "tags": {},
                "hdDeviceSettings": { "deviceState": "IDLE" },
            }),
        );
    }
}

/// Rule: a describe operation is a REPORT, so it writes nothing a later caller
/// could observe — repeating it over an unchanged store answers exactly the same
/// resource, and every other reader of that resource still reports it the same
/// way.
///
/// The rows differ along which calls wrote the device's state and along how many
/// times it is read, because the rule is about reading whatever is stored rather
/// than about any one device; each row's device is settled first, so what is
/// compared is the stored state alone.
#[tokio::test]
async fn repeating_the_read_answers_the_same_device_and_changes_nothing() {
    let table = [
        (
            "a device only claimed",
            "hd-repeat0000001",
            claimed_only(),
            2usize,
        ),
        (
            "a configured, named device",
            "uhd-repeat000001",
            Setup {
                update: Some(json!({
                    "name": "repeat-link",
                    "availabilityZone": "us-west-2c",
                    "uhdDeviceSettings": { "configuredInput": "HDMI", "maxBitrate": 15_000_000 },
                })),
                ..claimed_only()
            },
            5,
        ),
        (
            "a streaming device two inputs ingest from",
            "hd-repeat0000002",
            Setup {
                inputs: 2,
                streaming: true,
                tags: &[("team", "broadcast")],
                ..claimed_only()
            },
            8,
        ),
    ];

    for (case, id, setup, reads) in table {
        let router = medialive();
        apply(&router, case, id, &setup).await;
        // Everything the setup's calls owed the device has come due, so the rows
        // compare the stored device and not a transition in flight.
        wait_for_the_clock();

        let first = device(&router, case, id).await;
        assert_identity(case, id, &first);

        for read in 1..=reads {
            let again = device(&router, case, id).await;
            assert_eq!(
                again, first,
                "case {case:?}: expected read {read} to answer the same device, got {again}"
            );
        }

        // The other reader of the same record still reports it the same way, so
        // the repeated reads consumed nothing the listing would have seen.
        let (status, listed) = call(&router, "GET", "/prod/inputDevices", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInputDevices: expected 200, got {status}: {listed}"
        );
        let summary = listed["inputDevices"]
            .as_array()
            .and_then(|devices| devices.iter().find(|summary| summary["id"] == json!(id)))
            .unwrap_or_else(|| panic!("case {case:?}: expected {id} in the listing, got {listed}"));
        for member in MEMBERS {
            assert_eq!(
                summary.get(member).cloned().unwrap_or(Value::Null),
                first.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the listed {member} to equal the described one, got {:?}",
                summary.get(member)
            );
        }
    }
}

/// Rule: the only state a reporting operation may materialise is what the CLOCK
/// had already made due at the instant it ran, so reading a resource inside a
/// transition neither completes nor extends it: the transient value stands until
/// the span the service owes has passed, and the settled value from then on,
/// however often the resource is read in between.
///
/// Both transitions an input device owes the clock are rows — the maintenance
/// window that puts its software in `UPDATING` and the configuration change it
/// reports `SYNCING` until it has taken — because the rule is about a clock-owed
/// transition and not about either member.
#[tokio::test]
async fn reads_inside_a_transition_neither_complete_nor_extend_it() {
    // (case, the device, the setup that starts the transition, the member it
    // moves, its transient value, its settled value)
    let table = [
        (
            "a maintenance window on an hd device",
            "hd-transient0001",
            Setup {
                maintenance_window: true,
                ..claimed_only()
            },
            "deviceUpdateStatus",
            "UPDATING",
            "UP_TO_DATE",
        ),
        (
            "a configuration sent to an hd device",
            "hd-transient0002",
            Setup {
                update: Some(json!({ "hdDeviceSettings": { "maxBitrate": 12_000_000 } })),
                ..claimed_only()
            },
            "deviceSettingsSyncState",
            "SYNCING",
            "SYNCED",
        ),
        (
            "a configuration sent to a uhd device",
            "uhd-transient001",
            Setup {
                update: Some(json!({ "uhdDeviceSettings": { "configuredInput": "SDI" } })),
                ..claimed_only()
            },
            "deviceSettingsSyncState",
            "SYNCING",
            "SYNCED",
        ),
    ];

    for (case, id, setup, member, transient, settled) in table {
        let router = medialive();
        apply(&router, case, id, &setup).await;

        // Every read taken inside the span reports the transient value: the
        // transition is the clock's, so no number of reads brings it forward.
        let inside = device(&router, case, id).await;
        for read in 1..=4 {
            let again = device(&router, case, id).await;
            assert_eq!(
                again[member],
                json!(transient),
                "case {case:?}: expected read {read} inside the transition to still report \
                 {transient}, got {:?}",
                again[member]
            );
            assert_eq!(
                again, inside,
                "case {case:?}: expected read {read} to answer the same device, got {again}"
            );
        }

        wait_for_the_clock();
        let after = device(&router, case, id).await;
        assert_eq!(
            after[member],
            json!(settled),
            "case {case:?}: expected the due transition to report {settled}, got {:?}",
            after[member]
        );
        // The transition moved that member and nothing else.
        for (name, value) in inside.as_object().expect("the describe body is an object") {
            if name == member {
                continue;
            }
            assert_eq!(
                &after[name], value,
                "case {case:?}: expected the transition to leave {name} alone, got {:?}",
                after[name]
            );
        }
    }
}

/// Rule: a describe operation addresses a resource this account holds, so an
/// identifier no accepted call registered is answered with the operation's own
/// modelled `NotFoundException` (404) — carrying this service's not-found
/// wording, resource type then identifier — and reports no resource.
///
/// The ids differ in prefix, length and character class, include a near miss of
/// a claimed id, one that is not of the documented device-id form at all and one
/// that differs from a claimed id only in case (a device id is the unit's own,
/// so it is matched as spelled), because the rule is about resolution against
/// the store rather than about any one id shape.
#[tokio::test]
async fn describing_a_device_the_account_has_not_claimed_is_not_found() {
    let router = medialive();
    // Devices do exist, so the refusals are about the id and not about an empty
    // store.
    for claimed in ["hd-claimed000001", "UHD-Claimed0001"] {
        claim_device(&router, "claimed", claimed).await;
    }
    let empty = medialive();

    for id in [
        "hd-123456789abcdef",
        "uhd-0a1b2c3d",
        "hd-claimed00000",
        "hd-claimed0000012",
        "uhd-claimed0001",
        "not-a-device",
    ] {
        for (store, case) in [(&router, "a store holding other devices"), (&empty, "an empty store")] {
            let (status, body) = describe_device(store, id).await;
            assert_eq!(
                status, 404,
                "case {case:?}/{id:?}: expected 404 for an unclaimed device, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?}/{id:?}: expected the operation's modelled NotFoundException, \
                 got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("InputDevice {id} not found"),
                "case {case:?}/{id:?}: expected the message to name the resource type then the \
                 identifier, got {:?}",
                error_message(&body)
            );
            for member in ["id", "arn", "type"] {
                assert!(
                    body.get(member).is_none(),
                    "case {case:?}/{id:?}: expected a refusal to report no device, got {body}"
                );
            }
        }
    }

    // The refused reads left the devices that do exist exactly as they were.
    for claimed in ["hd-claimed000001", "UHD-Claimed0001"] {
        let described = device(&router, "claimed", claimed).await;
        assert_identity("claimed", claimed, &described);
    }
}
