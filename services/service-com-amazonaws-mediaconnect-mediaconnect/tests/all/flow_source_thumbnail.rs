// SPDX-License-Identifier: CC0-1.0

//! DescribeFlowSourceThumbnail, end to end over HTTP.
//!
//! The rule under test: the operation REPORTS what this service holds for the flow its
//! `{FlowArn}` label resolves to in the store — so it answers `NotFoundException` for a
//! label the store does not hold, and for a flow it does hold it answers that STORED flow's
//! ARN plus one `thumbnailMessages` entry per STORED source, coded from the flow's stored
//! source monitoring configuration and its stored status. Nothing in this emulator ingests
//! media, so no frame has ever been received: `thumbnail`, `timecode` and `timestamp` are
//! absent for every flow rather than carrying a synthesised image.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over —
//! whether the flow carries a source monitoring configuration at all, which state that
//! configuration names for thumbnails, which OTHER monitoring settings it carries, how many
//! sources the flow holds and what they are named, which protocol they use, and which status
//! the flow is in — rather than over one recorded request. Every case drives the real
//! request path (routing, restJson1 deserialization, constraint validation, handler,
//! serialization) through the harness [`crate::flow_lifecycle::Emulator`], so what these
//! tests assert is what a MediaConnect client sees on the wire.

use std::time::Duration;

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_source_metadata::{assert_messages_are_worded, coded, create};

/// The `MessageDetail$Code` values this operation reports, written out here rather than read
/// from the crate so that a change to the handler's constants cannot quietly move what these
/// tests assert.
///
/// `THUMBNAIL_DISABLED` is the code for a source of a flow whose stored
/// `MonitoringConfig$ThumbnailState` is `DISABLED`, `FLOW_NOT_RUNNING` the code for a source
/// of a flow that is not `ACTIVE`, and `NO_THUMBNAIL_AVAILABLE` the code for a source of a
/// running flow nothing has arrived on.
const CODE_THUMBNAIL_DISABLED: &str = "THUMBNAIL_DISABLED";
/// See [`CODE_THUMBNAIL_DISABLED`].
const CODE_FLOW_NOT_RUNNING: &str = "FLOW_NOT_RUNNING";
/// See [`CODE_THUMBNAIL_DISABLED`].
const CODE_NO_THUMBNAIL_AVAILABLE: &str = "NO_THUMBNAIL_AVAILABLE";

/// The status a created flow waits in, and the one a started flow settles into
/// (`smithy_query(query="shape",shape="Status")`).
const STATUS_STANDBY: &str = "STANDBY";
/// See [`STATUS_STANDBY`].
const STATUS_ACTIVE: &str = "ACTIVE";

/// Longer than the span the clock settles a flow's status over, so a read after it sees the
/// settled status.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// Call DescribeFlowSourceThumbnail through its modelled HTTP binding
/// (`smithy_query(query="operation",operation="DescribeFlowSourceThumbnail")`: GET
/// `/v1/flows/{FlowArn}/source-thumbnail`).
async fn source_thumbnail(emulator: &Emulator, flow_arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/flows/{flow_arn}/source-thumbnail"), None)
        .await
}

/// Start the flow and wait the clock out, so it has settled into `ACTIVE`.
async fn start_and_settle(emulator: &Emulator, case: &str, flow_arn: &str) {
    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: StartFlow expected 202, got {status} {code:?}: {body}"
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        described["flow"]["status"].as_str(),
        Some(STATUS_ACTIVE),
        "case {case:?}: the started flow must have settled into {STATUS_ACTIVE}, got {}",
        described["flow"]["status"]
    );
}

/// Assert that no image, timecode or generation instant is reported: nothing is ingested, so
/// no frame has ever been decoded for any flow.
fn assert_no_image_is_reported(case: &str, details: &Value) {
    for (member, value) in [
        ("thumbnail", &details["thumbnail"]),
        ("timecode", &details["timecode"]),
        ("timestamp", &details["timestamp"]),
    ] {
        assert!(
            value.is_null(),
            "case {case:?}: no {member} is reported for a flow nothing has been ingested \
             on, got {value}"
        );
    }
}

/// One row of the reporting table: a CreateFlow body, whether to run the flow before
/// reading, and the `MessageDetail$Code` the rule says every message of the report carries.
struct ReportCase {
    case: &'static str,
    /// The flow as the create request states it.
    flow: Value,
    /// Whether to start the flow (and wait the clock out) before reading its thumbnail.
    run: bool,
    /// The code every `thumbnailMessages` entry must carry.
    code: &'static str,
}

/// The rows span what the rule generalises over: a flow with no `sourceMonitoringConfig` at
/// all, one whose configuration names each value of the modelled `ThumbnailState` enum
/// (`smithy_query(query="shape",shape="ThumbnailState")`: ENABLED | DISABLED), one whose
/// configuration carries only OTHER monitoring settings and never mentions thumbnails, the
/// two statuses a flow's own lifecycle settles in (`STANDBY` and `ACTIVE`) crossed with each
/// of those configurations, how many sources the flow holds (one, and the two a flow under
/// active failover may hold), how they are named, and which protocol they use — including an
/// entitled source, which has no ingest endpoint of its own.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "a standby flow with no source monitoring configuration",
            flow: json!({
                "name": "thumbnail-no-config",
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.20.0.0/16" }
            }),
            run: false,
            code: CODE_FLOW_NOT_RUNNING,
        },
        ReportCase {
            case: "a standby flow with thumbnails ENABLED",
            flow: json!({
                "name": "thumbnail-enabled-standby",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": {
                    "name": "primary-feed",
                    "protocol": "srt-listener",
                    "whitelistCidr": "192.0.2.0/24",
                    "minLatency": 2000
                }
            }),
            run: false,
            code: CODE_FLOW_NOT_RUNNING,
        },
        ReportCase {
            case: "a running flow with thumbnails ENABLED",
            flow: json!({
                "name": "thumbnail-enabled-running",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.21.0.0/16" }
            }),
            run: true,
            code: CODE_NO_THUMBNAIL_AVAILABLE,
        },
        ReportCase {
            case: "a standby flow with thumbnails DISABLED",
            flow: json!({
                "name": "thumbnail-disabled-standby",
                "sourceMonitoringConfig": { "thumbnailState": "DISABLED" },
                "source": { "name": "src", "protocol": "rist", "whitelistCidr": "10.22.0.0/16" }
            }),
            run: false,
            code: CODE_THUMBNAIL_DISABLED,
        },
        ReportCase {
            // Thumbnails being off is not about what the flow receives: a RUNNING flow with
            // them off still has none.
            case: "a running flow with thumbnails DISABLED",
            flow: json!({
                "name": "thumbnail-disabled-running",
                "sourceMonitoringConfig": {
                    "thumbnailState": "DISABLED",
                    "contentQualityAnalysisState": "ENABLED"
                },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.23.0.0/16" }
            }),
            run: true,
            code: CODE_THUMBNAIL_DISABLED,
        },
        ReportCase {
            // A configuration that tunes other monitoring and never mentions thumbnails
            // states nothing about them, so the flow is reported on by its status alone.
            case: "a standby flow whose monitoring configuration never mentions thumbnails",
            flow: json!({
                "name": "thumbnail-other-settings",
                "sourceMonitoringConfig": {
                    "audioMonitoringSettings": [
                        { "silentAudio": { "state": "ENABLED", "thresholdSeconds": 12 } }
                    ],
                    "videoMonitoringSettings": [
                        { "blackFrames": { "state": "ENABLED", "thresholdSeconds": 4 } }
                    ]
                },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.24.0.0/16" }
            }),
            run: false,
            code: CODE_FLOW_NOT_RUNNING,
        },
        ReportCase {
            case: "a running flow whose monitoring configuration never mentions thumbnails",
            flow: json!({
                "name": "thumbnail-other-settings-running",
                "sourceMonitoringConfig": { "contentQualityAnalysisState": "DISABLED" },
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.25.0.0/16" }
            }),
            run: true,
            code: CODE_NO_THUMBNAIL_AVAILABLE,
        },
        ReportCase {
            case: "two sources under active failover, with thumbnails ENABLED",
            flow: json!({
                "name": "thumbnail-two-sources",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "sources": [
                    { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.26.0.0/16" },
                    { "name": "feed-b", "protocol": "rist", "whitelistCidr": "10.27.0.0/16" }
                ],
                "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "MERGE" }
            }),
            run: false,
            code: CODE_FLOW_NOT_RUNNING,
        },
        ReportCase {
            case: "two sources under active failover, with thumbnails DISABLED",
            flow: json!({
                "name": "thumbnail-two-sources-off",
                "sourceMonitoringConfig": { "thumbnailState": "DISABLED" },
                "sources": [
                    { "name": "feed-a", "protocol": "rtp", "whitelistCidr": "10.28.0.0/16" },
                    { "name": "feed-b", "protocol": "rtp", "whitelistCidr": "10.29.0.0/16" }
                ],
                "sourceFailoverConfig": { "state": "ENABLED" }
            }),
            run: false,
            code: CODE_THUMBNAIL_DISABLED,
        },
        ReportCase {
            case: "an entitled source, which has no ingest endpoint of its own",
            flow: json!({
                "name": "thumbnail-entitled",
                "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
                "source": {
                    "name": "entitled-feed",
                    "entitlementArn": format!(
                        "arn:aws:mediaconnect:{REGION}:444455556666:entitlement:1-0000beef:ent"
                    )
                }
            }),
            run: false,
            code: CODE_FLOW_NOT_RUNNING,
        },
    ]
}

/// DescribeFlowSourceThumbnail reports the STORED flow: its ARN, one coded message per
/// stored source naming that source, and no image, timecode or generation instant for any
/// flow — because nothing has been ingested.
#[tokio::test]
async fn source_thumbnail_reports_the_stored_flow_and_its_sources() {
    let emulator = Emulator::new();

    for ReportCase {
        case,
        flow,
        run,
        code,
    } in report_cases()
    {
        let (flow_arn, source_names) = create(&emulator, case, flow).await;
        assert!(
            !source_names.is_empty(),
            "case {case:?}: the fixture flow must hold at least one source"
        );
        if run {
            start_and_settle(&emulator, case, &flow_arn).await;
        }

        let (status, error, body) = source_thumbnail(&emulator, &flow_arn).await;
        assert_eq!(
            (status, error.as_deref()),
            (200, None),
            "case {case:?}: expected 200 with no error, got {status} {error:?}: {body}"
        );
        let details = &body["thumbnailDetails"];
        assert_eq!(
            details["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the details name the flow the call was performed on, got {body}"
        );

        // One message per stored source, in the flow's own source order, each naming its
        // source and carrying the code the stored configuration and status imply.
        let expected: Vec<(String, String)> = source_names
            .iter()
            .map(|name| (code.to_string(), name.clone()))
            .collect();
        assert_eq!(
            coded(&details["thumbnailMessages"]),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            details["thumbnailMessages"]
        );
        assert_messages_are_worded(case, &details["thumbnailMessages"]);
        assert_no_image_is_reported(case, details);
    }
}

/// The report follows the flow's STORED source monitoring configuration, not the request
/// that created it: turning thumbnails off through UpdateFlow and back on again moves the
/// code the SAME flow is reported with, each time read back through
/// DescribeFlowSourceThumbnail.
#[tokio::test]
async fn the_report_follows_the_stored_monitoring_configuration() {
    let emulator = Emulator::new();
    let case = "a flow whose monitoring configuration is updated";
    let (flow_arn, source_names) = create(
        &emulator,
        case,
        json!({
            "name": "thumbnail-updated",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.30.0.0/16" }
        }),
    )
    .await;

    /// One step: the `sourceMonitoringConfig` UpdateFlow is given (or `None` to read the
    /// flow as created), and the code the report must carry afterwards.
    struct Step {
        case: &'static str,
        config: Option<Value>,
        code: &'static str,
    }

    let steps = [
        Step {
            case: "as created, with no monitoring configuration",
            config: None,
            code: CODE_FLOW_NOT_RUNNING,
        },
        Step {
            case: "thumbnails turned off",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            code: CODE_THUMBNAIL_DISABLED,
        },
        Step {
            case: "thumbnails turned back on",
            config: Some(json!({ "thumbnailState": "ENABLED" })),
            code: CODE_FLOW_NOT_RUNNING,
        },
        Step {
            case: "the configuration replaced by one that never mentions thumbnails",
            config: Some(json!({ "contentQualityAnalysisState": "ENABLED" })),
            code: CODE_FLOW_NOT_RUNNING,
        },
        Step {
            case: "thumbnails turned off again",
            config: Some(json!({ "thumbnailState": "DISABLED" })),
            code: CODE_THUMBNAIL_DISABLED,
        },
    ];

    for Step { case, config, code } in steps {
        if let Some(config) = config {
            let (status, error, body) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "sourceMonitoringConfig": config })),
                )
                .await;
            assert_eq!(
                (status, error.as_deref()),
                (202, None),
                "case {case:?}: UpdateFlow expected 202, got {status} {error:?}: {body}"
            );
        }

        let (status, error, body) = source_thumbnail(&emulator, &flow_arn).await;
        assert_eq!(
            (status, error.as_deref()),
            (200, None),
            "case {case:?}: expected 200 with no error, got {status} {error:?}: {body}"
        );
        let expected: Vec<(String, String)> = source_names
            .iter()
            .map(|name| (code.to_string(), name.clone()))
            .collect();
        assert_eq!(
            coded(&body["thumbnailDetails"]["thumbnailMessages"]),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            body["thumbnailDetails"]["thumbnailMessages"]
        );
        assert_no_image_is_reported(case, &body["thumbnailDetails"]);
    }
}

/// The read REPORTS: calling it twice answers the same body, and it moves neither the flow's
/// status nor its monitoring configuration, so a later caller sees exactly what an earlier
/// one did.
#[tokio::test]
async fn source_thumbnail_is_a_report_that_changes_nothing() {
    let emulator = Emulator::new();
    let case = "repeated reads of one flow";
    let (flow_arn, _) = create(
        &emulator,
        case,
        json!({
            "name": "thumbnail-stable",
            "sourceMonitoringConfig": { "thumbnailState": "ENABLED" },
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.31.0.0/16" }
        }),
    )
    .await;

    let (_, _, before) = emulator.describe_flow(&flow_arn).await;
    let (_, _, first) = source_thumbnail(&emulator, &flow_arn).await;
    let (_, _, second) = source_thumbnail(&emulator, &flow_arn).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of an unchanged flow answer the same body, got {first} \
         then {second}"
    );

    let (status, error, after) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, error.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {error:?}: {after}"
    );
    assert_eq!(
        after["flow"]["status"].as_str(),
        Some(STATUS_STANDBY),
        "case {case:?}: reading the thumbnail did not move the flow's status, got {}",
        after["flow"]["status"]
    );
    assert_eq!(
        before, after,
        "case {case:?}: reading the thumbnail changed nothing about the flow, got {before} \
         then {after}"
    );
}

/// The `{FlowArn}` label is resolved against the STORE once it satisfies the constraint the
/// model states for it: a label of the modelled form that the store does not hold is refused
/// 404 `NotFoundException` whose message is the label the request named, while a label
/// outside that form is refused by the modelled constraint itself (400 `BadRequestException`)
/// before any lookup.
///
/// `DescribeFlowSourceThumbnailRequest$FlowArn` declares no pattern of its own
/// (`smithy_query(query="operation",operation="DescribeFlowSourceThumbnail")`), so the narrow
/// `FlowArn` shape pattern is the only one the model states for it.
#[tokio::test]
async fn an_unheld_flow_label_is_refused_not_found() {
    let emulator = Emulator::new();

    // A flow that existed and does not any more resolves to nothing, like one that never
    // did.
    let (deleted, _) = create(
        &emulator,
        "deleted flow",
        json!({
            "name": "thumbnail-doomed",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.32.0.0/16" }
        }),
    )
    .await;
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{deleted}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the fixture flow expected 202, got {status} {code:?}: {body}"
    );

    // Labels of the modelled form that address no flow of this store: one deleted, one
    // never minted, one of another account, one of another Region.
    let unheld = [
        ("a flow that was deleted", deleted),
        (
            "a flow ARN of the modelled form that names no flow",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-made"),
        ),
        (
            "a flow ARN of another account",
            format!("arn:aws:mediaconnect:{REGION}:999988887777:flow:1-0000beef:elsewhere"),
        ),
        (
            "a flow ARN of another Region",
            "arn:aws:mediaconnect:eu-west-1:111122223333:flow:1-0000beef:elsewhere".to_string(),
        ),
    ];
    for (case, label) in unheld {
        let (status, code, body) = source_thumbnail(&emulator, &label).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                service_com_amazonaws_mediaconnect_mediaconnect::state::not_found_message(&label)
                    .as_str()
            ),
            "case {case:?}: the message is the label the request named in this service's \
             envelope, got {body}"
        );
    }

    // Labels outside the modelled form, one broken segment at a time: the modelled
    // constraint refuses them before the store is consulted.
    let malformed = [
        ("not an ARN at all", "not-a-flow".to_string()),
        (
            "an ARN of another resource type",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:source:1-0000dead:src"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:medialive:{REGION}:111122223333:flow:1-0000dead:f"),
        ),
        (
            "an account segment that is not twelve digits",
            format!("arn:aws:mediaconnect:{REGION}:11112222333:flow:1-0000dead:f"),
        ),
        (
            "a flow name outside the pattern's character class",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:f.1"),
        ),
    ];
    for (case, label) in malformed {
        let (status, code, body) = source_thumbnail(&emulator, &label).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("flowArn"),
            "case {case:?}: the refusal names the member it judged, got {message:?}"
        );
    }
}
