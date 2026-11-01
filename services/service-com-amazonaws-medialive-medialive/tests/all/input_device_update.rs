// SPDX-License-Identifier: CC0-1.0

//! `UpdateInputDevice` — the parameters a caller sets on a claimed input device,
//! end to end over the restJson1 wire, together with the `ClaimDevice` that
//! brings the device into the account and the `DescribeInputDevice` that reads
//! the update back.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables spanning what the rules generalise over: the device's
//! TYPE (the `InputDeviceType` values the model declares), which members the
//! request names, and the bounds the settings' own documentation states. The
//! `hd-123456789abcdef`-shaped device the operation documents by example is ONE
//! row. The wire harness (`medialive`, `call`, `error_code`), the claim and the
//! describe helpers and the clock wait are the crate's shared ones — reused, not
//! copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_medialive_medialive::state::{
    repeated_audio_channel_pair_message, FIRST_AUDIO_CHANNEL_PAIR_ID, LAST_AUDIO_CHANNEL_PAIR_ID,
    MAX_AUDIO_CHANNEL_PAIRS,
};

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_device_maintenance_window::{claim_device, describe_device, device};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateInputDevice` as a client calls it: `PUT /prod/inputDevices/{id}`.
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/prod/inputDevices/{id}"),
        Some(body),
    )
    .await
}

/// One accepted update, asserting the call succeeded and handing back what it
/// reported.
async fn updated(router: &MultiServiceRouter, case: &str, id: &str, body: Value) -> Value {
    let (status, response) = update(router, id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateInputDevice({id}): expected 200, got {status}: {response}"
    );
    response
}

/// The device as the update reported it and as `DescribeInputDevice` reports it
/// afterwards, asserted to be the same projection of the same record.
///
/// `DeviceSettingsSyncState` is the one member left out of the comparison: it is
/// what the CLOCK owes a configured device, so the two reads differ on it
/// exactly when the synchronisation came due between them.
async fn update_and_read_back(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    body: Value,
) -> (Value, Value) {
    let reported = updated(router, case, id, body).await;
    let described = device(router, case, id).await;
    for (member, value) in described.as_object().expect("the describe body is an object") {
        if member == "deviceSettingsSyncState" {
            continue;
        }
        assert_eq!(
            &reported[member], value,
            "case {case:?}: expected the update to report {member} as a later read does, got {:?}",
            reported[member]
        );
    }
    (reported, described)
}

/// Every member of `expected` as the device reports it.
fn assert_members(case: &str, described: &Value, expected: &Value) {
    for (member, value) in expected.as_object().expect("the expectation is an object") {
        assert_eq!(
            &described[member], value,
            "case {case:?}: expected {member} to be {value}, got {:?}",
            described[member]
        );
    }
}

/// One row of the update table: the device to claim, the request body and what
/// the device must report afterwards.
struct Row {
    case: &'static str,
    id: &'static str,
    body: Value,
    expected: Value,
}

/// Rule: `UpdateInputDevice` applies the members its request NAMES to the device
/// its id resolves to — the name and the availability zone MediaLive records for
/// it, and the configurable settings of the device's own type — persists them,
/// and reports the device back out of that stored state, so a later read reports
/// exactly the same device; a member the request omits is left as it was.
///
/// The rows differ along what the rule generalises over: the device's type (both
/// `InputDeviceType` values), which members the request names (none, the
/// MediaLive-side members, the settings members shared by both types, the
/// UHD-only ones) and the values those members carry, because the rule is about
/// applying whatever the request names rather than about any one member or
/// device.
#[tokio::test]
async fn an_update_applies_the_members_the_request_names() {
    // The configurable settings' values are the model's own enums
    // (`smithy_query(query="shape", shape="InputDeviceConfiguredInput")`: AUTO |
    // HDMI | SDI; `shape="InputDeviceCodec"`: HEVC | AVC;
    // `shape="InputDeviceConfigurableAudioChannelPairProfile"`, whose DISABLED
    // and CBR-AAC_HQ-192000 are two of its eight), and the audio pair ids are
    // the published bounds of "a value from 1 to 8".
    let table: [Row; 7] = [
        Row {
            case: "a request that names no member at all",
            id: "hd-123456789abcdef",
            body: json!({}),
            expected: json!({
                "id": "hd-123456789abcdef",
                "type": "HD",
                "deviceSettingsSyncState": "SYNCED",
                "hdDeviceSettings": { "deviceState": "IDLE" },
                "outputType": "NONE",
            }),
        },
        Row {
            case: "the name the account gives the device",
            id: "hd-123456789abcdef",
            body: json!({ "name": "studio-link-1" }),
            expected: json!({
                "name": "studio-link-1",
                // Naming the device changes no DEVICE setting, so nothing is
                // sent to the unit.
                "deviceSettingsSyncState": "SYNCED",
                "hdDeviceSettings": { "deviceState": "IDLE" },
            }),
        },
        Row {
            case: "a name and an availability zone",
            id: "uhd-0a1b2c3d4e5f6071",
            body: json!({ "name": "truck-link", "availabilityZone": "us-west-2b" }),
            expected: json!({
                "name": "truck-link",
                "availabilityZone": "us-west-2b",
                "type": "UHD",
                "deviceSettingsSyncState": "SYNCED",
                "uhdDeviceSettings": { "deviceState": "IDLE" },
            }),
        },
        Row {
            case: "the settings an hd device can be configured with",
            id: "hd-00112233445566778899aabbccddeeff",
            body: json!({
                "hdDeviceSettings": { "configuredInput": "SDI", "maxBitrate": 20_000_000, "latencyMs": 1_000 },
            }),
            expected: json!({
                "hdDeviceSettings": {
                    "configuredInput": "SDI",
                    "deviceState": "IDLE",
                    "maxBitrate": 20_000_000,
                    "latencyMs": 1_000,
                },
                "deviceSettingsSyncState": "SYNCING",
            }),
        },
        Row {
            case: "the settings a uhd device can be configured with, including the uhd-only ones",
            id: "UHD-A1B2",
            body: json!({
                "name": "uhd-link",
                "uhdDeviceSettings": {
                    "configuredInput": "AUTO",
                    "maxBitrate": 30_000_000,
                    "latencyMs": 2_000,
                    "codec": "HEVC",
                    "inputResolution": "UHD",
                    "audioChannelPairs": [
                        { "id": 1, "profile": "CBR-AAC_HQ-192000" },
                        { "id": 8, "profile": "DISABLED" },
                    ],
                },
            }),
            expected: json!({
                "name": "uhd-link",
                "uhdDeviceSettings": {
                    "configuredInput": "AUTO",
                    "deviceState": "IDLE",
                    "maxBitrate": 30_000_000,
                    "latencyMs": 2_000,
                    "codec": "HEVC",
                    "inputResolution": "UHD",
                    "audioChannelPairs": [
                        { "id": 1, "profile": "CBR-AAC_HQ-192000" },
                        { "id": 8, "profile": "DISABLED" },
                    ],
                },
                "deviceSettingsSyncState": "SYNCING",
            }),
        },
        Row {
            case: "a mediaconnect flow attached to a uhd device",
            id: "uhd-flowattached01",
            body: json!({
                "uhdDeviceSettings": {
                    "mediaconnectSettings": {
                        "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:link-flow",
                        "roleArn": "arn:aws:iam::000000000000:role/MediaLiveAccessRole",
                        "secretArn": "arn:aws:secretsmanager:us-west-2:000000000000:secret:link-key",
                        "sourceName": "link-source",
                    },
                },
            }),
            expected: json!({
                // "Specifies MEDIACONNECT_FLOW if this device is the source for
                // a MediaConnect flow."
                "outputType": "MEDIACONNECT_FLOW",
                "uhdDeviceSettings": {
                    "deviceState": "IDLE",
                    "mediaconnectSettings": {
                        "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:link-flow",
                        "roleArn": "arn:aws:iam::000000000000:role/MediaLiveAccessRole",
                        "secretArn": "arn:aws:secretsmanager:us-west-2:000000000000:secret:link-key",
                        "sourceName": "link-source",
                    },
                },
                "deviceSettingsSyncState": "SYNCING",
            }),
        },
        Row {
            case: "the lowest bitrate and latency a request can carry",
            id: "hd-zeroconfigured1",
            body: json!({ "hdDeviceSettings": { "configuredInput": "HDMI", "maxBitrate": 0, "latencyMs": 0 } }),
            expected: json!({
                "hdDeviceSettings": {
                    "configuredInput": "HDMI",
                    "deviceState": "IDLE",
                    "maxBitrate": 0,
                    "latencyMs": 0,
                },
                "deviceSettingsSyncState": "SYNCING",
            }),
        },
    ];

    for Row {
        case,
        id,
        body,
        expected,
    } in table
    {
        let router = medialive();
        // A second device nothing updates, so the suite can tell one device's
        // update from a store-wide change.
        let bystander = "hd-bystander0001";
        claim_device(&router, case, bystander).await;
        claim_device(&router, case, id).await;
        let claimed = device(&router, case, bystander).await;

        let (_, described) = update_and_read_back(&router, case, id, body).await;
        assert_members(case, &described, &expected);

        assert_eq!(
            device(&router, case, bystander).await,
            claimed,
            "case {case:?}: expected the update to leave the device it did not name alone"
        );
    }
}

/// Rule: an update names what it CHANGES — each member it carries replaces the
/// stored value and each member it omits leaves it — except the MediaConnect
/// attachment, whose own documentation makes a present value a wholesale one
/// ("To attach this device to a MediaConnect flow, specify these parameters. To
/// detach an existing flow, enter {} for the value of mediaconnectSettings").
///
/// One device is carried through a sequence of updates, because the rule is
/// about what a LATER update does to what an earlier one stored.
#[tokio::test]
async fn successive_updates_merge_and_an_empty_mediaconnect_settings_detaches() {
    let case = "successive updates";
    let router = medialive();
    let id = "uhd-merge000000001";
    claim_device(&router, case, id).await;

    updated(
        &router,
        case,
        id,
        json!({
            "name": "first-name",
            "availabilityZone": "us-west-2a",
            "uhdDeviceSettings": { "configuredInput": "SDI", "codec": "AVC", "maxBitrate": 10_000_000 },
        }),
    )
    .await;

    // A second update that names only the bitrate keeps everything the first
    // one stored.
    let (_, merged) = update_and_read_back(
        &router,
        case,
        id,
        json!({ "uhdDeviceSettings": { "maxBitrate": 25_000_000 } }),
    )
    .await;
    assert_members(
        case,
        &merged,
        &json!({
            "name": "first-name",
            "availabilityZone": "us-west-2a",
            "uhdDeviceSettings": {
                "configuredInput": "SDI",
                "codec": "AVC",
                "deviceState": "IDLE",
                "maxBitrate": 25_000_000,
            },
        }),
    );

    // Attaching a flow makes the device that flow's source.
    let flow_arn = "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:merge-flow";
    let (_, attached) = update_and_read_back(
        &router,
        case,
        id,
        json!({ "uhdDeviceSettings": { "mediaconnectSettings": { "flowArn": flow_arn } } }),
    )
    .await;
    assert_members(
        case,
        &attached,
        &json!({
            "outputType": "MEDIACONNECT_FLOW",
            "uhdDeviceSettings": {
                "configuredInput": "SDI",
                "codec": "AVC",
                "deviceState": "IDLE",
                "maxBitrate": 25_000_000,
                "mediaconnectSettings": { "flowArn": flow_arn },
            },
        }),
    );

    // And an empty `mediaconnectSettings` detaches it again, which leaves the
    // device attached to nothing and reports no flow.
    let (_, detached) = update_and_read_back(
        &router,
        case,
        id,
        json!({ "uhdDeviceSettings": { "mediaconnectSettings": {} } }),
    )
    .await;
    assert_members(
        case,
        &detached,
        &json!({
            "outputType": "NONE",
            "uhdDeviceSettings": {
                "configuredInput": "SDI",
                "codec": "AVC",
                "deviceState": "IDLE",
                "maxBitrate": 25_000_000,
            },
        }),
    );
}

/// Rule: a configuration change is SENT to the device, so a device an update has
/// just configured reports `DeviceSettingsSyncState` `SYNCING` ("it has not
/// updated its configuration") from the moment the call returns until the clock
/// makes the synchronisation due, and `SYNCED` ("the device has updated its
/// configuration") afterwards. An update that changes no device setting leaves
/// the state alone.
///
/// Both device types are exercised, because the rule is about the configuration
/// a device takes rather than about one type's settings.
#[tokio::test]
async fn a_configured_device_is_syncing_until_the_clock_syncs_it() {
    for (case, id, member) in [
        ("an hd device", "hd-syncing000001", "hdDeviceSettings"),
        ("a uhd device", "uhd-syncing00001", "uhdDeviceSettings"),
    ] {
        let router = medialive();
        claim_device(&router, case, id).await;
        assert_eq!(
            device(&router, case, id).await["deviceSettingsSyncState"],
            json!("SYNCED"),
            "case {case:?}: expected a device no update has configured to hold no pending change"
        );

        let reported = updated(
            &router,
            case,
            id,
            json!({ member: { "maxBitrate": 12_000_000 } }),
        )
        .await;
        assert_eq!(
            reported["deviceSettingsSyncState"],
            json!("SYNCING"),
            "case {case:?}: expected the update itself to report the unsynchronised change, got {:?}",
            reported["deviceSettingsSyncState"]
        );
        assert_eq!(
            device(&router, case, id).await["deviceSettingsSyncState"],
            json!("SYNCING"),
            "case {case:?}: expected a read straight after the update to report SYNCING"
        );

        wait_for_the_clock();
        assert_eq!(
            device(&router, case, id).await["deviceSettingsSyncState"],
            json!("SYNCED"),
            "case {case:?}: expected the device to have taken the configuration once it came due"
        );

        // An update that names no device setting sends the unit nothing.
        let named = updated(&router, case, id, json!({ "name": "renamed" })).await;
        assert_eq!(
            named["deviceSettingsSyncState"],
            json!("SYNCED"),
            "case {case:?}: expected naming the device to send it nothing, got {:?}",
            named["deviceSettingsSyncState"]
        );
    }
}

/// Rule: an update is refused, with the operation's modelled
/// `BadRequestException` (400), when it carries settings the device it names
/// cannot take — the settings member of the other `InputDeviceType`, a member
/// only a UHD device has ("Only UHD devices can specify this parameter"), more
/// audio configurations than a source has pairs, an audio pair id outside "a
/// value from 1 to 8", or a source resolution outside "(HD or UHD)" — and
/// nothing is written for it.
///
/// The rows span each refused class, each published bound and one past it, so
/// the rule is asserted over the class rather than over one request.
#[tokio::test]
async fn settings_the_device_cannot_take_are_refused() {
    let hd = "hd-refused000001";
    let uhd = "uhd-refused00001";

    // (case, the device the update addresses, the request body)
    let table: [(&str, &str, Value); 10] = [
        (
            "uhd settings on an hd device",
            hd,
            json!({ "uhdDeviceSettings": { "maxBitrate": 1_000 } }),
        ),
        (
            "hd settings on a uhd device",
            uhd,
            json!({ "hdDeviceSettings": { "maxBitrate": 1_000 } }),
        ),
        (
            "a codec on an hd device",
            hd,
            json!({ "hdDeviceSettings": { "codec": "AVC" } }),
        ),
        (
            "a source resolution on an hd device",
            hd,
            json!({ "hdDeviceSettings": { "inputResolution": "HD" } }),
        ),
        (
            "audio pair profiles on an hd device",
            hd,
            json!({ "hdDeviceSettings": { "audioChannelPairs": [{ "id": 1, "profile": "DISABLED" }] } }),
        ),
        (
            "a mediaconnect flow on an hd device",
            hd,
            json!({ "hdDeviceSettings": { "mediaconnectSettings": { "flowArn": "arn:aws:mediaconnect:us-west-2:000000000000:flow:1-aBcDeF:f" } } }),
        ),
        (
            "an audio pair id below the first one",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [{ "id": 0, "profile": "DISABLED" }] } }),
        ),
        (
            "an audio pair id past the last one",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [{ "id": 9, "profile": "DISABLED" }] } }),
        ),
        (
            "one audio configuration more than a source has pairs",
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": 1 }, { "id": 2 }, { "id": 3 }, { "id": 4 },
                { "id": 5 }, { "id": 6 }, { "id": 7 }, { "id": 8 }, { "id": 8 },
            ] } }),
        ),
        (
            "a source resolution the member does not name",
            uhd,
            json!({ "uhdDeviceSettings": { "inputResolution": "FHD" } }),
        ),
    ];

    for (case, id, body) in table {
        let router = medialive();
        claim_device(&router, case, hd).await;
        claim_device(&router, case, uhd).await;
        let before = device(&router, case, id).await;

        let (status, response) = update(&router, id, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {case:?}: expected the operation's modelled BadRequestException, got {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case:?}: expected the refusal to say what the caller must change, got {response}"
        );

        assert_eq!(
            device(&router, case, id).await,
            before,
            "case {case:?}: expected the refused update to write nothing"
        );
    }
}

/// Rule: `AudioChannelPairs` configures the audio pairs of ONE source — "An
/// array of eight audio configurations, one for each audio pair in the source" —
/// so each pair the list names is a pair it configures once: a list that names
/// one pair more than once leaves that pair's profile undecided and is refused
/// with the operation's modelled `BadRequestException` (400), naming the
/// repeated pair, and writes nothing. A list that names each of its pairs once
/// is accepted however many pairs it carries, in whatever order, with whatever
/// profiles.
///
/// The rows differ along what the rule generalises over: WHICH pair repeats
/// (both published bounds of "a value from 1 to 8" and one in between), how
/// often it repeats, where in the list the repeat falls, whether the repeated
/// entries carry the same profile, a differing one or none, and how long the
/// list is — because the rule is about one pair being configured twice rather
/// than about any one id, profile, position or length.
#[tokio::test]
async fn one_audio_configuration_for_each_audio_pair() {
    let uhd = "uhd-audiopairs001";

    // (case, the request body, the pair the refusal must name)
    let refused: [(&str, Value, i32); 6] = [
        (
            "the first pair configured twice with two different profiles",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": FIRST_AUDIO_CHANNEL_PAIR_ID, "profile": "CBR-AAC_HQ-192000" },
                { "id": FIRST_AUDIO_CHANNEL_PAIR_ID, "profile": "DISABLED" },
            ] } }),
            FIRST_AUDIO_CHANNEL_PAIR_ID,
        ),
        (
            "the last pair configured twice with the same profile",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": LAST_AUDIO_CHANNEL_PAIR_ID, "profile": "DISABLED" },
                { "id": LAST_AUDIO_CHANNEL_PAIR_ID, "profile": "DISABLED" },
            ] } }),
            LAST_AUDIO_CHANNEL_PAIR_ID,
        ),
        (
            "a pair configured twice with another pair's configuration between them",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": 2, "profile": "VBR-AAC_HE-64000" },
                { "id": 3, "profile": "VBR-AAC_LC-128000" },
                { "id": 2, "profile": "CBR-AAC_HQ-256000" },
            ] } }),
            2,
        ),
        (
            "a pair configured three times",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": 5, "profile": "DISABLED" },
                { "id": 5, "profile": "CBR-AAC_HQ-384000" },
                { "id": 5, "profile": "CBR-AAC_HQ-512000" },
            ] } }),
            5,
        ),
        (
            "a pair configured twice by entries that carry no profile at all",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [{ "id": 4 }, { "id": 4 }] } }),
            4,
        ),
        (
            "as many configurations as a source has pairs, one pair twice and one not at all",
            json!({ "uhdDeviceSettings": { "audioChannelPairs": [
                { "id": 1, "profile": "DISABLED" }, { "id": 2, "profile": "DISABLED" },
                { "id": 3, "profile": "DISABLED" }, { "id": 4, "profile": "DISABLED" },
                { "id": 5, "profile": "DISABLED" }, { "id": 6, "profile": "DISABLED" },
                { "id": 7, "profile": "DISABLED" }, { "id": 7, "profile": "VBR-AAC_HHE-16000" },
            ] } }),
            7,
        ),
    ];

    for (case, body, repeated) in refused {
        let router = medialive();
        claim_device(&router, case, uhd).await;
        let before = device(&router, case, uhd).await;

        let (status, response) = update(&router, uhd, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status}: {response}"
        );
        assert_eq!(
            error_code(status, &response),
            "BadRequestException",
            "case {case:?}: expected the operation's modelled BadRequestException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            repeated_audio_channel_pair_message(repeated),
            "case {case:?}: expected the refusal to name audio pair {repeated}, got {:?}",
            error_message(&response)
        );

        assert_eq!(
            device(&router, case, uhd).await,
            before,
            "case {case:?}: expected the refused update to write nothing"
        );
    }

    // Every profile is one the model's own enum declares
    // (`smithy_query(query="shape",
    // shape="InputDeviceConfigurableAudioChannelPairProfile")`: DISABLED |
    // VBR-AAC_HHE-16000 | VBR-AAC_HE-64000 | VBR-AAC_LC-128000 |
    // CBR-AAC_HQ-192000 | CBR-AAC_HQ-256000 | CBR-AAC_HQ-384000 |
    // CBR-AAC_HQ-512000), and every id is inside "a value from 1 to 8".
    let accepted: [(&str, Value); 4] = [
        (
            "one pair configured once",
            json!([{ "id": 6, "profile": "VBR-AAC_HHE-16000" }]),
        ),
        (
            "every pair a source has, each configured once",
            json!([
                { "id": 1, "profile": "DISABLED" },
                { "id": 2, "profile": "VBR-AAC_HHE-16000" },
                { "id": 3, "profile": "VBR-AAC_HE-64000" },
                { "id": 4, "profile": "VBR-AAC_LC-128000" },
                { "id": 5, "profile": "CBR-AAC_HQ-192000" },
                { "id": 6, "profile": "CBR-AAC_HQ-256000" },
                { "id": 7, "profile": "CBR-AAC_HQ-384000" },
                { "id": 8, "profile": "CBR-AAC_HQ-512000" },
            ]),
        ),
        (
            "distinct pairs listed from the last to the first",
            json!([
                { "id": 8, "profile": "DISABLED" },
                { "id": 7, "profile": "DISABLED" },
                { "id": 6, "profile": "CBR-AAC_HQ-512000" },
            ]),
        ),
        (
            "distinct pairs that all carry the same profile",
            json!([
                { "id": 2, "profile": "VBR-AAC_HE-64000" },
                { "id": 4, "profile": "VBR-AAC_HE-64000" },
                { "id": 8, "profile": "VBR-AAC_HE-64000" },
            ]),
        ),
    ];

    for (case, pairs) in accepted {
        let router = medialive();
        claim_device(&router, case, uhd).await;

        let (_, described) = update_and_read_back(
            &router,
            case,
            uhd,
            json!({ "uhdDeviceSettings": { "audioChannelPairs": pairs.clone() } }),
        )
        .await;
        assert_eq!(
            described["uhdDeviceSettings"]["audioChannelPairs"], pairs,
            "case {case:?}: expected the device to report the configurations it was given, got {:?}",
            described["uhdDeviceSettings"]["audioChannelPairs"]
        );
        assert!(
            pairs
                .as_array()
                .expect("the configurations are an array")
                .len()
                <= MAX_AUDIO_CHANNEL_PAIRS,
            "case {case:?}: expected the accepted rows to stay inside the {MAX_AUDIO_CHANNEL_PAIRS} pairs a source has, got {}",
            pairs.as_array().expect("the configurations are an array").len()
        );
    }
}

/// Rule: an input device operation addresses a device this account has CLAIMED,
/// so an update of an id no claim registered is answered with the operation's
/// own modelled `NotFoundException` (404), carrying this service's not-found
/// wording — resource type then identifier — and nothing is written for it.
///
/// The ids differ in prefix, length and character class, and include one that is
/// not of the documented device-id form at all, because the rule is about
/// resolution against the store rather than about any one id shape.
#[tokio::test]
async fn updating_a_device_the_account_has_not_claimed_is_not_found() {
    let router = medialive();
    // One device exists, so the refusals are about the id and not about an empty
    // store.
    let claimed = "hd-claimed000001";
    claim_device(&router, "claimed", claimed).await;
    let before = device(&router, "claimed", claimed).await;

    for id in [
        "hd-123456789abcdef",
        "uhd-0a1b2c3d",
        "hd-claimed00000",
        "not-a-device",
    ] {
        let (status, response) = update(&router, id, json!({ "name": "nowhere" })).await;
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
            "case {id:?}: expected the message to name the resource type then the identifier, got {:?}",
            error_message(&response)
        );

        let (status, described) = describe_device(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected the refused id to still name no device, got {status}: {described}"
        );
    }

    assert_eq!(
        device(&router, "claimed", claimed).await,
        before,
        "expected the refused updates to leave the claimed device alone"
    );
}
