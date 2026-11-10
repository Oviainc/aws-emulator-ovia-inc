// SPDX-License-Identifier: CC0-1.0

//! UpdateFlow over HTTP: the partial update of a stored flow's settings.
//!
//! The rule under test is one sentence: UpdateFlow applies exactly the members the
//! request carries to the stored flow — a member present replaces that setting, a member
//! absent leaves the stored setting untouched — and answers with the stored record, which
//! is the record DescribeFlow then reports.
//!
//! The table below is the class of that rule rather than one request: its rows differ in
//! WHICH members a request carries (one, several, none), in the kind of member (a scalar
//! enum, a wrapper shape merged member-wise, a whole configuration structure) and in
//! whether the setting being written already had a stored value. Every row is applied to
//! the SAME flow in sequence, so each row also asserts that the settings the previous
//! rows wrote are still there.
//!
//! Requests go through the same `MultiServiceRouter` the emulator's front door uses, so
//! what is asserted here is what a MediaConnect client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;

/// The six flow-level settings UpdateFlow can write, by their restJson1 member names
/// (`smithy_query(query="operation",operation="UpdateFlow")`). Every expectation below
/// names all six, so a row asserts both what it changed and what it left alone.
const SETTINGS: [&str; 6] = [
    "flowSize",
    "maintenance",
    "sourceFailoverConfig",
    "sourceMonitoringConfig",
    "ndiConfig",
    "encodingConfig",
];

/// The members of a flow UpdateFlow models no input for, so no row may move them.
const UNTOUCHED: [&str; 7] = [
    "flowArn",
    "name",
    "status",
    "availabilityZone",
    "egressIp",
    "source",
    "sources",
];

/// One row of the UpdateFlow table: a request body, and the six settings the stored flow
/// must hold once it has been applied.
struct UpdateCase {
    case: &'static str,
    body: Value,
    expected: Value,
}

/// Assert the six settings of `flow` are exactly `expected`.
fn assert_settings(case: &str, flow: &Value, expected: &Value) {
    for member in SETTINGS {
        assert_eq!(
            flow[member], expected[member],
            "case {case:?}: setting {member:?} after the update, got {:?}",
            flow[member]
        );
    }
}

/// The updates applied, in order, to a flow created MEDIUM with a failover pair of
/// sources named `primary` / `backup`.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "no member at all: an update that changes nothing",
            body: json!({}),
            expected: json!({
                "flowSize": "MEDIUM",
                "maintenance": null,
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "FAILOVER",
                    "recoveryWindow": 3000
                },
                "sourceMonitoringConfig": null,
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            case: "one scalar enum member: the flow size",
            body: json!({ "flowSize": "LARGE" }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": null,
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "FAILOVER",
                    "recoveryWindow": 3000
                },
                "sourceMonitoringConfig": null,
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            case: "a maintenance window where the flow had none",
            body: json!({
                "maintenance": { "maintenanceDay": "Tuesday", "maintenanceStartHour": "13:00" }
            }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "FAILOVER",
                    "recoveryWindow": 3000
                },
                "sourceMonitoringConfig": null,
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            // `UpdateMaintenance`'s members are each optional, so a request that carries
            // only the scheduled date keeps the day and hour the flow already holds.
            case: "one member of the maintenance wrapper merges into the stored window",
            body: json!({ "maintenance": { "maintenanceScheduledDate": "2026-03-14" } }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "FAILOVER",
                    "recoveryWindow": 3000
                },
                "sourceMonitoringConfig": null,
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            case: "two members of the failover wrapper merge into the stored config",
            body: json!({
                "sourceFailoverConfig": {
                    "failoverMode": "MERGE",
                    "sourcePriority": { "primarySource": "backup" }
                }
            }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "MERGE",
                    "recoveryWindow": 3000,
                    "sourcePriority": { "primarySource": "backup" }
                },
                "sourceMonitoringConfig": null,
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            // `SourceMonitoringConfig` targets the stored `MonitoringConfig` shape itself,
            // so the value in the request IS the new setting, lists and all.
            case: "a whole monitoring configuration, with a nested list",
            body: json!({
                "sourceMonitoringConfig": {
                    "thumbnailState": "ENABLED",
                    "contentQualityAnalysisState": "DISABLED",
                    "videoMonitoringSettings": [
                        { "blackFrames": { "state": "ENABLED", "thresholdSeconds": 4 } }
                    ]
                }
            }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "MERGE",
                    "recoveryWindow": 3000,
                    "sourcePriority": { "primarySource": "backup" }
                },
                "sourceMonitoringConfig": {
                    "thumbnailState": "ENABLED",
                    "contentQualityAnalysisState": "DISABLED",
                    "videoMonitoringSettings": [
                        { "blackFrames": { "state": "ENABLED", "thresholdSeconds": 4 } }
                    ]
                },
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            // The monitoring configuration is replaced, not merged: the previous value's
            // `videoMonitoringSettings` do not survive a request that omits them.
            case: "a second monitoring configuration replaces the first one whole",
            body: json!({
                "sourceMonitoringConfig": {
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ]
                }
            }),
            expected: json!({
                "flowSize": "LARGE",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "MERGE",
                    "recoveryWindow": 3000,
                    "sourcePriority": { "primarySource": "backup" }
                },
                "sourceMonitoringConfig": {
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ]
                },
                "ndiConfig": null,
                "encodingConfig": null
            }),
        },
        UpdateCase {
            case: "three members at once: NDI, its encoding profile and a larger flow",
            body: json!({
                "flowSize": "LARGE_4X",
                "ndiConfig": {
                    "ndiState": "ENABLED",
                    "machineName": "studio-a",
                    "ndiDiscoveryServers": [
                        {
                            "discoveryServerAddress": "198.51.100.40",
                            "discoveryServerPort": 5959,
                            "vpcInterfaceAdapter": "vpc-1"
                        }
                    ]
                },
                "encodingConfig": {
                    "encodingProfile": "CONTRIBUTION_H264_DEFAULT",
                    "videoMaxBitrate": 40_000_000
                }
            }),
            expected: json!({
                "flowSize": "LARGE_4X",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "MERGE",
                    "recoveryWindow": 3000,
                    "sourcePriority": { "primarySource": "backup" }
                },
                "sourceMonitoringConfig": {
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ]
                },
                "ndiConfig": {
                    "ndiState": "ENABLED",
                    "machineName": "studio-a",
                    "ndiDiscoveryServers": [
                        {
                            "discoveryServerAddress": "198.51.100.40",
                            "discoveryServerPort": 5959,
                            "vpcInterfaceAdapter": "vpc-1"
                        }
                    ]
                },
                "encodingConfig": {
                    "encodingProfile": "CONTRIBUTION_H264_DEFAULT",
                    "videoMaxBitrate": 40_000_000
                }
            }),
        },
        UpdateCase {
            case: "an encoding configuration replaces the stored one whole",
            body: json!({ "encodingConfig": { "encodingProfile": "DISTRIBUTION_H264_DEFAULT" } }),
            expected: json!({
                "flowSize": "LARGE_4X",
                "maintenance": {
                    "maintenanceDay": "Tuesday",
                    "maintenanceStartHour": "13:00",
                    "maintenanceScheduledDate": "2026-03-14"
                },
                "sourceFailoverConfig": {
                    "state": "ENABLED",
                    "failoverMode": "MERGE",
                    "recoveryWindow": 3000,
                    "sourcePriority": { "primarySource": "backup" }
                },
                "sourceMonitoringConfig": {
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ]
                },
                "ndiConfig": {
                    "ndiState": "ENABLED",
                    "machineName": "studio-a",
                    "ndiDiscoveryServers": [
                        {
                            "discoveryServerAddress": "198.51.100.40",
                            "discoveryServerPort": 5959,
                            "vpcInterfaceAdapter": "vpc-1"
                        }
                    ]
                },
                "encodingConfig": { "encodingProfile": "DISTRIBUTION_H264_DEFAULT" }
            }),
        },
    ]
}

/// One update per row: the response is the stored record, DescribeFlow reports the same
/// record afterwards, and nothing the request did not name has moved.
#[tokio::test]
async fn update_flow_writes_the_members_the_request_carries_and_only_those() {
    let emulator = Emulator::new();
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "flow-under-update",
            "flowSize": "MEDIUM",
            "sources": [
                { "name": "primary", "protocol": "zixi-push", "whitelistCidr": "10.1.0.0/16" },
                { "name": "backup", "protocol": "zixi-push", "whitelistCidr": "10.2.0.0/16" }
            ],
            "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "FAILOVER", "recoveryWindow": 3000 }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "create expected 201 with no error, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();
    let before: Vec<Value> = UNTOUCHED
        .iter()
        .map(|member| created["flow"][member].clone())
        .collect();

    for UpdateCase {
        case,
        body,
        expected,
    } in update_cases()
    {
        let (status, code, updated) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body.clone()))
            .await;
        // `UpdateFlow`'s modelled success status
        // (`smithy_query(query="operation",operation="UpdateFlow")`: http code 202).
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {updated}"
        );
        assert_settings(case, &updated["flow"], &expected);

        // The response is the STORED record: a read afterwards reports the same bytes.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200 with no error, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["flow"], updated["flow"],
            "case {case:?}: DescribeFlow must report the record UpdateFlow answered with, got {:?}",
            described["flow"]
        );

        // Nothing UpdateFlow models no input for has moved.
        for (member, value) in UNTOUCHED.iter().zip(&before) {
            assert_eq!(
                &described["flow"][member], value,
                "case {case:?}: UpdateFlow must not touch {member:?}, got {:?}",
                described["flow"][member]
            );
        }
    }
}

/// One row of the refusal table: a request body and the modelled error it must answer.
struct RefusalCase {
    case: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
}

/// UpdateFlow refuses a request that breaks a rule of the class with the error the model
/// declares for it, and a refused request writes NOTHING.
#[tokio::test]
async fn update_flow_refusals_name_the_modelled_error_and_write_nothing() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "flow-refusals",
            "flowSize": "MEDIUM",
            "sources": [
                { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                { "name": "backup", "protocol": "rtp", "whitelistCidr": "10.3.0.0/16" }
            ],
            "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "MERGE" }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    // Well-formed per the modelled `FlowArn` pattern, but no such flow.
    let missing = "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:missing";
    let (status, code, body) = emulator
        .send(
            "PUT",
            &format!("/v1/flows/{missing}"),
            Some(json!({ "flowSize": "LARGE" })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"update an unknown flow\": expected 404 NotFoundException, got {status} \
         {code:?}: {body}"
    );

    let cases = vec![
        RefusalCase {
            // This operation's own doc: a MEDIUM flow must be upgraded to LARGE before
            // its source can become NDI.
            case: "NDI enabled on a MEDIUM flow",
            body: json!({ "ndiConfig": { "ndiState": "ENABLED" } }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "NDI enabled while the same request downgrades the flow to MEDIUM",
            body: json!({
                "flowSize": "MEDIUM",
                "ndiConfig": { "ndiState": "ENABLED", "machineName": "studio-b" }
            }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // `UpdateFailoverConfig$State`: "if the state is inactive, the flow can have
            // only one source" — and this flow ingests a pair.
            case: "failover disabled under a failover pair of sources",
            body: json!({ "sourceFailoverConfig": { "state": "DISABLED" } }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // `SourcePriority$PrimarySource` is "the name of the source you choose as the
            // primary source for this flow".
            case: "a primary source the flow does not have",
            body: json!({
                "sourceFailoverConfig": { "sourcePriority": { "primarySource": "not-a-source" } }
            }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // Refused by the modelled `FlowSize` enum before the handler runs, and
            // reported as this service's own client-input error (the refusal override in
            // `src/service.rs`), so an unknown enum value is a 400, never a 501.
            case: "a flow size the model does not declare",
            body: json!({ "flowSize": "ENORMOUS" }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // `UpdateMaintenance$MaintenanceStartHour`: "use 24-hour HH:MM format. Minutes
            // must be 00."
            case: "a maintenance start hour whose minutes are not 00",
            body: json!({ "maintenance": { "maintenanceStartHour": "13:30" } }),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // `UpdateMaintenance$MaintenanceScheduledDate`: "use YYYY-MM-DD format."
            case: "a maintenance scheduled date that is not ISO YYYY-MM-DD",
            body: json!({ "maintenance": { "maintenanceScheduledDate": "03/14/2026" } }),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, answered) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body.clone()))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             {code:?}: {answered}"
        );

        // A refused update leaves the stored flow exactly as it was.
        let (status, code, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: describe expected 200 with no error, got {status} {code:?}: {described}"
        );
        assert_eq!(
            described["flow"], created["flow"],
            "case {case:?}: a refused update must write nothing, got {:?}",
            described["flow"]
        );
    }
}

/// A maintenance window that names a day but no hour happens at the hour the model
/// documents as the default (`UpdateMaintenance$MaintenanceStartHour`: "the default value
/// is 02:00"), and that stored default survives a later partial update.
#[tokio::test]
async fn a_maintenance_window_without_an_hour_takes_the_documented_default() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "flow-maintenance",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    for (case, body, day, hour) in [
        (
            "a day with no hour",
            json!({ "maintenance": { "maintenanceDay": "Sunday" } }),
            "Sunday",
            "02:00",
        ),
        (
            "a later request naming only the day keeps the stored hour",
            json!({ "maintenance": { "maintenanceDay": "Friday" } }),
            "Friday",
            "02:00",
        ),
        (
            "an explicit hour replaces the default",
            json!({ "maintenance": { "maintenanceStartHour": "23:00" } }),
            "Friday",
            "23:00",
        ),
    ] {
        let (status, code, updated) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {updated}"
        );
        assert_eq!(
            (
                updated["flow"]["maintenance"]["maintenanceDay"].as_str(),
                updated["flow"]["maintenance"]["maintenanceStartHour"].as_str(),
            ),
            (Some(day), Some(hour)),
            "case {case:?}: maintenance day and start hour, got {:?}",
            updated["flow"]["maintenance"]
        );
    }
}

/// NDI needs the larger flow, so enabling it is accepted exactly on the flow sizes that
/// support it.
///
/// The rows are the WHOLE `FlowSize` catalog
/// (`smithy_query(query="shape",shape="FlowSize")`: MEDIUM | LARGE | LARGE_4X) plus the
/// flow whose size was never set, each created as its own flow, so the rule is asserted
/// over every published value of the enum rather than over the one the issue named.
#[tokio::test]
async fn enabling_ndi_is_accepted_only_on_an_ndi_capable_flow_size() {
    let emulator = Emulator::new();

    for (case, flow_size, expected_status, expected_code) in [
        ("no flow size stored", None, 400, Some("BadRequestException")),
        ("MEDIUM", Some("MEDIUM"), 400, Some("BadRequestException")),
        ("LARGE", Some("LARGE"), 202, None),
        ("LARGE_4X", Some("LARGE_4X"), 202, None),
    ] {
        let mut body = json!({
            "name": format!("flow-ndi-{}", flow_size.unwrap_or("unset")),
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        });
        if let Some(flow_size) = flow_size {
            body["flowSize"] = json!(flow_size);
        }
        let (status, _, created) = emulator.create_flow(body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create expected 201, got {status}: {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .expect("the created flow carries its ARN")
            .to_string();

        let (status, code, answered) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{flow_arn}"),
                Some(json!({ "ndiConfig": { "ndiState": "ENABLED" } })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code:?}, got {status} \
             {code:?}: {answered}"
        );

        // Accepted or refused, the store and the answer agree. An accepted update stores
        // the state it named PLUS the prefix the service assigns a config that named none
        // (asserted as a rule, over a span of inputs, in `crate::flow_ndi_machine_name`).
        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        let stored = described["flow"]["ndiConfig"].clone();
        if expected_status == 202 {
            assert_eq!(
                stored["ndiState"],
                json!("ENABLED"),
                "case {case:?}: stored NdiConfig state, got {stored:?}"
            );
            assert!(
                crate::flow_ndi_machine_name::generated_id_rejection(
                    stored["machineName"].as_str().unwrap_or_default()
                )
                .is_none(),
                "case {case:?}: a stored NdiConfig carries the generated prefix the service \
                 assigns a config that named none, got {stored:?}"
            );
        } else {
            assert_eq!(
                stored,
                Value::Null,
                "case {case:?}: a refused update stores no NdiConfig, got {stored:?}"
            );
        }
    }
}

/// A flow whose SOURCE is NDI® keeps an NDI-capable size: "if you're switching from an
/// NDI source to a transport stream (TS) source and want to downgrade the flow size:
/// first, use the UpdateFlowSource operation to change the flow source type" (UpdateFlow's
/// own documentation). Only a request that changes the size is refused for it.
#[tokio::test]
async fn a_flow_with_an_ndi_source_keeps_an_ndi_capable_size() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "flow-ndi-source",
            "flowSize": "LARGE",
            "source": {
                "name": "ndi-in",
                // `Protocol` (`smithy_query(query="shape",shape="Protocol")`) and
                // `NdiSourceSettings`, whose audio reference level is in -40..=0 dBFS.
                "protocol": "ndi-speed-hq",
                "ndiSourceSettings": { "sourceName": "STUDIO (Cam1)", "audioReferenceLevel": -20 }
            }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    for (case, body, expected_status, expected_code) in [
        (
            "downgrade to the size NDI cannot run on",
            json!({ "flowSize": "MEDIUM" }),
            400,
            Some("BadRequestException"),
        ),
        (
            "a request that does not touch the size",
            json!({ "maintenance": { "maintenanceDay": "Monday" } }),
            202,
            None,
        ),
        (
            "an upgrade to the largest size",
            json!({ "flowSize": "LARGE_4X" }),
            202,
            None,
        ),
        (
            "a downgrade is still refused after the upgrade",
            json!({ "flowSize": "MEDIUM" }),
            400,
            Some("BadRequestException"),
        ),
    ] {
        let (status, code, answered) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code:?}, got {status} \
             {code:?}: {answered}"
        );
    }

    let (_, _, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        described["flow"]["flowSize"].as_str(),
        Some("LARGE_4X"),
        "the refused downgrades wrote nothing, got {:?}",
        described["flow"]["flowSize"]
    );
}

/// The two maintenance members the request shape leaves free-form carry documented
/// formats: `MaintenanceStartHour` is "24-hour HH:MM format. Minutes must be 00" and
/// `MaintenanceScheduledDate` is "YYYY-MM-DD format"
/// (`smithy_query(query="shape",shape="UpdateMaintenance")`). Each row is on a published
/// bound of the range, or one step past it.
#[tokio::test]
async fn a_maintenance_window_must_carry_the_documented_formats() {
    let emulator = Emulator::new();
    let (status, _, created) = emulator
        .create_flow(json!({
            "name": "flow-maintenance-formats",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }))
        .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    let hours = [
        ("the bottom of the hour range", "00:00", true),
        ("the top of the hour range", "23:00", true),
        ("the documented example hour", "13:00", true),
        ("one hour past the top of the range", "24:00", false),
        ("minutes that are not 00", "13:30", false),
        ("an hour that is not two digits", "7:00", false),
        ("an hour that is not a time at all", "noon", false),
    ];
    let dates = [
        ("the documented example date", "2021-01-30", true),
        ("the last day of a year", "2026-12-31", true),
        ("the first day of a year", "2026-01-01", true),
        ("one month past the top of the range", "2026-13-01", false),
        ("the day below the bottom of the range", "2026-02-00", false),
        ("one day past the top of the range", "2026-02-32", false),
        ("a month that is not two digits", "2026-3-14", false),
        ("a date in another notation", "03/14/2026", false),
    ];

    for (case, body, accepted) in hours
        .iter()
        .map(|(case, hour, accepted)| {
            (
                *case,
                json!({ "maintenance": { "maintenanceStartHour": hour } }),
                *accepted,
            )
        })
        .chain(dates.iter().map(|(case, date, accepted)| {
            (
                *case,
                json!({ "maintenance": { "maintenanceScheduledDate": date } }),
                *accepted,
            )
        }))
    {
        let (status, code, answered) = emulator
            .send("PUT", &format!("/v1/flows/{flow_arn}"), Some(body.clone()))
            .await;
        let (expected_status, expected_code) = if accepted {
            (202, None)
        } else {
            (400, Some("BadRequestException"))
        };
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?} ({body}): expected {expected_status} {expected_code:?}, got \
             {status} {code:?}: {answered}"
        );
    }
}
