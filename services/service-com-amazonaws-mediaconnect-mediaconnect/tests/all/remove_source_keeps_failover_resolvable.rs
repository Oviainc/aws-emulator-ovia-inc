// SPDX-License-Identifier: CC0-1.0

//! Removing a source may not leave the resource's stored failover setting naming a source
//! the resource has not got, end to end over HTTP.
//!
//! `SourcePriority$PrimarySource` is "the name of the source you choose as the primary
//! source for this flow"
//! (`smithy_query(query="doc",shape="SourcePriority",member="PrimarySource")`), so a stored
//! `SourceFailoverConfig` holds a reference to one of its resource's OWN sources —
//! the reference every path that WRITES a priority resolves against the sources the resource
//! holds (`crate::failover_primary_source` is that side of the rule). The removal side is
//! asserted here: a request to take away the source a stored priority names is refused with
//! the modelled `BadRequestException` (HTTP 400) in this service's validation envelope and
//! takes nothing away, exactly as a media stream a source still associates
//! (`crate::remove_flow_media_stream`) or a VPC interface a source still references
//! (`crate::remove_flow_vpc_interface`) is refused removal — so whatever a resource reports
//! as its failover setting is one a later update can re-state.
//!
//! The tables span what the rule generalises over: WHICH position of the pair the priority
//! names and which one the request removes (so the refusal follows the reference rather than
//! the position), the shape of the source names, whether a priority is stored at all, and —
//! on the bridge side — the other resource whose sources one `SourceFailoverConfig` governs.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_gateway_arn};
use crate::flow_lifecycle::Emulator;
use crate::flow_sources::{described_sources, remove_flow_source};

/// The failover setting a resource needs to hold a PAIR of sources — "if the state is
/// inactive, [it] can have only one source. If the state is active, [it] can have one or two
/// sources" (`FailoverConfig$State`) — naming `primary` where a row stores a priority.
fn failover(primary: Option<&str>) -> Value {
    let mut config = json!({ "state": mc::STATE_ENABLED, "failoverMode": "MERGE" });
    if let Some(primary) = primary {
        config["sourcePriority"] = json!({ "primarySource": primary });
    }
    config
}

/// One row: the pair of sources the resource holds, the primary source its stored failover
/// setting names, and which of the pair the request removes.
struct Row {
    case: &'static str,
    names: [&'static str; 2],
    primary: Option<&'static str>,
    removed: usize,
}

impl Row {
    /// Whether the removal must be refused — computed from the RULE (the request takes away
    /// the source the stored priority names), never listed per row.
    fn refused(&self) -> bool {
        self.primary == Some(self.names[self.removed])
    }

    /// The name of the source the request addresses.
    fn target(&self) -> &'static str {
        self.names[self.removed]
    }
}

/// The table both the flow side and the bridge side run.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the priority names the second source and the request removes it",
            names: ["primary", "backup"],
            primary: Some("backup"),
            removed: 1,
        },
        Row {
            case: "the priority names the second source and the request removes the first",
            names: ["primary", "backup"],
            primary: Some("backup"),
            removed: 0,
        },
        Row {
            case: "the priority names the first source and the request removes it",
            names: ["primary", "backup"],
            primary: Some("primary"),
            removed: 0,
        },
        Row {
            case: "the priority names the first source and the request removes the second",
            names: ["primary", "backup"],
            primary: Some("primary"),
            removed: 1,
        },
        Row {
            // Source names are opaque identifiers, so a pair carrying mixed case, a digit
            // and punctuation is as ordinary as any other.
            case: "names carrying mixed case, a digit and punctuation, the primary removed",
            names: ["Alpha_1", "beta-2"],
            primary: Some("Alpha_1"),
            removed: 0,
        },
        Row {
            case: "names carrying mixed case, a digit and punctuation, the other removed",
            names: ["Alpha_1", "beta-2"],
            primary: Some("Alpha_1"),
            removed: 1,
        },
        Row {
            case: "a failover setting that names no primary source, the first removed",
            names: ["primary", "backup"],
            primary: None,
            removed: 0,
        },
        Row {
            case: "a failover setting that names no primary source, the second removed",
            names: ["primary", "backup"],
            primary: None,
            removed: 1,
        },
    ]
}

/// Assert one refusal: the modelled error, this service's validation envelope, and a message
/// that states which source could not be removed and why.
fn assert_refused(case: &str, target: &str, status: u16, code: Option<&str>, answered: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX)
            && message.contains(&format!("{target:?}"))
            && message.contains("primary source"),
        "case {case:?}: the refusal names {target:?} as the primary source that cannot be \
         removed, got {message:?}"
    );
}

/// The `SourceFailoverConfig` the flow reports, as DescribeFlow answers it.
async fn flow_failover(emulator: &Emulator, case: &str, flow_arn: &str) -> Value {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["sourceFailoverConfig"].clone()
}

/// The names of the sources `described` holds, in order.
fn source_names(described: &[Value]) -> Vec<String> {
    described
        .iter()
        .map(|source| source["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The primary source `config` names, if it names one.
fn primary_of(config: &Value) -> Option<String> {
    config["sourcePriority"]["primarySource"]
        .as_str()
        .map(str::to_string)
}

/// Assert the invariant the whole rule exists for: the failover setting the resource reports
/// names one of the sources it reports, and the update path accepts that setting unchanged.
async fn assert_failover_is_resolvable(
    emulator: &Emulator,
    case: &str,
    uri: &str,
    config: &Value,
    names: &[String],
) {
    if let Some(primary) = primary_of(config) {
        assert!(
            names.contains(&primary),
            "case {case:?}: the stored failover setting names {primary:?} as the primary \
             source while the resource holds {names:?}"
        );
    }
    let (status, code, answered) = emulator
        .send("PUT", uri, Some(json!({ "sourceFailoverConfig": config })))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the stored failover setting {config} must be one the update path \
         accepts unchanged, got {status} {code:?}: {answered}"
    );
}

/// RemoveFlowSource refuses to take away the source the flow's stored failover setting names
/// as its primary source, and removes every other source of the pair.
#[tokio::test]
async fn remove_flow_source_refuses_to_strand_the_failover_primary_source() {
    let emulator = Emulator::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let sources: Vec<Value> = row
            .names
            .iter()
            .enumerate()
            .map(|(position, name)| {
                json!({
                    "name": name,
                    "protocol": "rtp",
                    "whitelistCidr": format!("10.{}.{position}.0/24", index + 1),
                })
            })
            .collect();
        let (status, code, created) = emulator
            .create_flow(json!({
                "name": format!("flow-failover-removal-{index}"),
                "sources": sources,
                "sourceFailoverConfig": failover(row.primary),
            }))
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the flow under test must be created, got {status} {code:?}: \
             {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        let before = described_sources(&emulator, case, &flow_arn).await;
        let failover_before = flow_failover(&emulator, case, &flow_arn).await;
        let target_arn = before[row.removed]["sourceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the flow's sources carry ARNs: {before:?}"))
            .to_string();

        let (status, code, answered) = remove_flow_source(&emulator, &flow_arn, &target_arn).await;
        let after = described_sources(&emulator, case, &flow_arn).await;
        let failover_after = flow_failover(&emulator, case, &flow_arn).await;

        if row.refused() {
            assert_refused(case, row.target(), status, code.as_deref(), &answered);
            assert_eq!(
                after, before,
                "case {case:?}: a refused remove drops nothing, got {:?}",
                source_names(&after)
            );
            assert_eq!(
                failover_after, failover_before,
                "case {case:?}: a refused remove writes no failover setting either, got \
                 {failover_after}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: a source no failover priority names is removable, got {status} \
                 {code:?}: {answered}"
            );
            let expected: Vec<String> = row
                .names
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != row.removed)
                .map(|(_, name)| (*name).to_string())
                .collect();
            assert_eq!(
                source_names(&after),
                expected,
                "case {case:?}: the flow keeps its other sources, in order, got {after:?}"
            );
            assert_eq!(
                failover_after, failover_before,
                "case {case:?}: removing another source leaves the failover setting as it \
                 was, got {failover_after}"
            );
        }

        // Whichever way the request went, what the flow now reports is resolvable: the
        // state the rule protects.
        assert_failover_is_resolvable(
            &emulator,
            case,
            &format!("/v1/flows/{flow_arn}"),
            &failover_after,
            &source_names(&after),
        )
        .await;
    }
}

/// The way out of a refusal, which is why refusing is enough: a caller that wants the
/// primary source gone points the priority at another source of the flow first, through the
/// update path that writes one, and the same remove is then accepted.
#[tokio::test]
async fn re_pointing_the_priority_makes_the_refused_removal_succeed() {
    let emulator = Emulator::new();
    let case = "the failover primary source, re-pointed then removed";
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "flow-failover-removal-repoint",
            "sources": [
                { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.20.0.0/16" },
                { "name": "backup", "protocol": "rtp", "whitelistCidr": "10.21.0.0/16" },
            ],
            "sourceFailoverConfig": failover(Some("backup")),
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string();
    let backup_arn = described_sources(&emulator, case, &flow_arn).await[1]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, answered) = remove_flow_source(&emulator, &flow_arn, &backup_arn).await;
    assert_refused(case, "backup", status, code.as_deref(), &answered);

    let (status, code, updated) = emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}"),
            Some(json!({ "sourceFailoverConfig": failover(Some("primary")) })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the priority may be pointed at another source of the flow, got \
         {status} {code:?}: {updated}"
    );

    let (status, code, removed) = remove_flow_source(&emulator, &flow_arn, &backup_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the source no priority names any more is removable, got {status} \
         {code:?}: {removed}"
    );
    let after = source_names(&described_sources(&emulator, case, &flow_arn).await);
    assert_eq!(
        after,
        vec!["primary".to_string()],
        "case {case:?}: the flow keeps the source the priority now names, got {after:?}"
    );
    let config = flow_failover(&emulator, case, &flow_arn).await;
    assert_eq!(
        primary_of(&config).as_deref(),
        Some("primary"),
        "case {case:?}: the flow reports the primary source the update named, got {config}"
    );
    assert_failover_is_resolvable(
        &emulator,
        case,
        &format!("/v1/flows/{flow_arn}"),
        &config,
        &after,
    )
    .await;
}

/// The `SourceFailoverConfig` the bridge reports, as DescribeBridge answers it, with the
/// names of the sources it holds.
async fn bridge_state(emulator: &Emulator, case: &str, arn: &str) -> (Value, Vec<String>) {
    let (status, code, described) = describe_bridge(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
    );
    let names = described["bridge"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|source| {
            source["networkSource"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    (described["bridge"]["sourceFailoverConfig"].clone(), names)
}

/// The bridge side of the same rule: a bridge's sources are named too and one
/// `SourceFailoverConfig` governs them, so RemoveBridgeSource refuses to take away the
/// source the stored priority names and removes any other.
#[tokio::test]
async fn remove_bridge_source_refuses_to_strand_the_failover_primary_source() {
    let emulator = Emulator::new();
    let placement = minted_gateway_arn(7, "gw-failover-removal");

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let sources: Vec<Value> = row
            .names
            .iter()
            .enumerate()
            .map(|(position, name)| {
                json!({ "networkSource": {
                    "multicastIp": "224.0.1.1",
                    "name": name,
                    "networkName": "net-1",
                    "port": 5_000 + (index * 2 + position) as i64,
                    "protocol": "rtp",
                }})
            })
            .collect();
        let (status, code, created) = create_bridge(
            &emulator,
            json!({
                "name": format!("bridge-failover-removal-{index}"),
                "placementArn": placement,
                "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 2 },
                "sources": sources,
                "sourceFailoverConfig": failover(row.primary),
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the bridge under test must be created, got {status} {code:?}: \
             {created}"
        );
        let bridge_arn = created["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();

        let (failover_before, before) = bridge_state(&emulator, case, &bridge_arn).await;
        let (status, code, answered) = emulator
            .send(
                "DELETE",
                &format!("/v1/bridges/{bridge_arn}/sources/{}", row.target()),
                None,
            )
            .await;
        let (failover_after, after) = bridge_state(&emulator, case, &bridge_arn).await;

        if row.refused() {
            assert_refused(case, row.target(), status, code.as_deref(), &answered);
            assert_eq!(
                after, before,
                "case {case:?}: a refused remove drops no bridge source, got {after:?}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: a bridge source no failover priority names is removable, got \
                 {status} {code:?}: {answered}"
            );
            let expected: Vec<String> = row
                .names
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != row.removed)
                .map(|(_, name)| (*name).to_string())
                .collect();
            assert_eq!(
                after, expected,
                "case {case:?}: the bridge keeps its other sources, in order, got {after:?}"
            );
        }
        assert_eq!(
            failover_after, failover_before,
            "case {case:?}: neither outcome rewrites the bridge's failover setting, got \
             {failover_after}"
        );
        assert_failover_is_resolvable(
            &emulator,
            case,
            &format!("/v1/bridges/{bridge_arn}"),
            &failover_after,
            &after,
        )
        .await;
    }
}
