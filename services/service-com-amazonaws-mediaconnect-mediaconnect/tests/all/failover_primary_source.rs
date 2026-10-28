// SPDX-License-Identifier: CC0-1.0

//! Which source a stored source-failover setting may name as its PRIMARY source, end to
//! end over HTTP.
//!
//! `SourcePriority$PrimarySource` is "the name of the source you choose as the primary
//! source for this flow"
//! (`smithy_query(query="doc",shape="SourcePriority",member="PrimarySource")`), so it is a
//! reference to one of the resource's OWN sources — exactly like the media-stream and
//! VPC-interface names a source or an output carries. So every request that writes a
//! source priority resolves that name against the sources the resource holds once the
//! request has been applied, and one that names anything else is refused with the modelled
//! `BadRequestException` (HTTP 400) in this service's validation envelope, storing nothing.
//!
//! The rule binds every operation that writes a priority, both on the resource whose
//! sources a `SourceFailoverConfig` governs: CreateFlow and CreateBridge resolve it against
//! the sources THAT request creates (all the sources a resource being created has), while
//! UpdateFlow and UpdateBridge resolve it against the sources the stored resource already
//! holds. Each has its rows below.
//!
//! The tables span what the rule generalises over: the member the sources arrive in
//! (`Source` / `Sources`), how many of them there are, the shape of their names, whether
//! the request named them at all (a source the SERVICE named is just as much a source of
//! the flow), and which name the priority points at — its own first or second source, a
//! name no source has, the same name in another case, the empty name, and a name that
//! belongs to ANOTHER resource of the same service.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::{create_bridge, describe_bridge, minted_gateway_arn};
use crate::flow_lifecycle::Emulator;

/// The failover state a flow or a bridge needs to hold a PAIR of sources: "if the state is
/// inactive, [it] can have only one source. If the state is active, [it] can have one or
/// two sources" (`FailoverConfig$State`). Every case below uses it, so the source count is
/// never what decides a row.
fn failover(priority: Option<&str>) -> Value {
    let mut config = json!({ "state": mc::STATE_ENABLED, "failoverMode": "MERGE" });
    if let Some(primary) = priority {
        config["sourcePriority"] = json!({ "primarySource": primary });
    }
    config
}

/// One form a request's sources can arrive in, and the names the resource holds for them.
struct SourceForm {
    /// What the form is called in a case name.
    label: &'static str,
    /// The request member the sources arrive in (`source` or `sources`), with its value.
    member: &'static str,
    sources: Value,
    /// The names the resource's sources end up with, in order.
    names: Vec<&'static str>,
}

/// The flow source forms the table runs, every one of them naming its sources explicitly.
/// A source the REQUEST did not name is the subject of its own case below, because the name
/// the service assigns one depends on the flow's name.
fn flow_forms() -> Vec<SourceForm> {
    vec![
        SourceForm {
            label: "a failover pair through Sources",
            member: "sources",
            sources: json!([
                { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" },
                { "name": "backup", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" },
            ]),
            names: vec!["primary", "backup"],
        },
        SourceForm {
            label: "one source through Source",
            member: "source",
            sources: json!({ "name": "only", "protocol": "udp", "whitelistCidr": "10.3.0.0/16" }),
            names: vec!["only"],
        },
        SourceForm {
            // Source names are opaque identifiers, so a row whose names carry an
            // underscore, a hyphen, a digit and mixed case is as ordinary as any other.
            label: "a pair whose names carry mixed case, a digit and punctuation",
            member: "sources",
            sources: json!([
                { "name": "Alpha_1", "protocol": "rtp-fec", "whitelistCidr": "10.4.0.0/16" },
                { "name": "beta-2", "protocol": "srt-listener", "whitelistCidr": "10.5.0.0/16" },
            ]),
            names: vec!["Alpha_1", "beta-2"],
        },
    ]
}

/// One primary-source candidate: what to put in `PrimarySource`, and whether the rule
/// accepts it for a resource whose sources are the ones the row was built from.
struct Candidate {
    case: String,
    primary: Option<String>,
    accepted: bool,
}

/// Every candidate worth putting to a resource whose sources are `names`, with `foreign` a
/// source name the service knows on ANOTHER resource.
///
/// A candidate is accepted exactly when it names one of `names` (or names nothing at all),
/// which is the rule under test — so the expectation is computed from the resource's own
/// sources rather than listed per row.
fn candidates(names: &[&str], foreign: &str) -> Vec<Candidate> {
    let mut rows = vec![Candidate {
        case: "no sourcePriority at all".to_string(),
        primary: None,
        accepted: true,
    }];
    for (index, name) in names.iter().enumerate() {
        rows.push(Candidate {
            case: format!("source {index} of the resource, {name:?}"),
            primary: Some((*name).to_string()),
            accepted: true,
        });
    }
    for primary in [
        "not-a-source".to_string(),
        names[0].to_uppercase(),
        String::new(),
        foreign.to_string(),
    ] {
        rows.push(Candidate {
            case: format!("the primary source {primary:?}"),
            accepted: names.contains(&primary.as_str()),
            primary: Some(primary),
        });
    }
    rows
}

/// Assert one refusal: the modelled error, this service's validation envelope, and a
/// message that states which name was unacceptable.
fn assert_refused(case: &str, primary: &str, status: u16, code: Option<&str>, answered: &Value) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX)
            && message.contains(&format!("{primary:?}"))
            && message.contains("primary source"),
        "case {case:?}: the refusal names {primary:?} as the unacceptable primary source, \
         got {message:?}"
    );
}

/// The `SourcePriority$PrimarySource` a flow reports, as DescribeFlow answers it.
async fn described_primary(emulator: &Emulator, case: &str, flow_arn: &str) -> Value {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["sourceFailoverConfig"]["sourcePriority"]["primarySource"].clone()
}

/// Whether a flow of this name is in the store at all, as ListFlows answers it.
async fn flow_is_listed(emulator: &Emulator, case: &str, name: &str) -> bool {
    let (status, _, listed) = emulator.list_flows("?maxResults=100").await;
    assert_eq!(status, 200, "case {case:?}: ListFlows must answer 200: {listed}");
    listed["flows"]
        .as_array()
        .is_some_and(|flows| flows.iter().any(|flow| flow["name"].as_str() == Some(name)))
}

/// CreateFlow resolves the primary source its request names against the sources THAT
/// request creates: it stores a priority that names one of them, and refuses — with
/// `BadRequestException` (400), storing no flow — one that names anything else.
///
/// Each accepted row is read back through DescribeFlow and then put to UpdateFlow
/// unchanged, which is the point of the rule: a create may not store a failover setting the
/// update path would refuse.
#[tokio::test]
async fn create_flow_resolves_the_primary_source_against_the_sources_it_creates() {
    let emulator = Emulator::new();

    // A source name the service knows, on a flow that is not the one under test.
    let neighbour = "neighbour-src";
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "primary-neighbour",
            "source": { "name": neighbour, "protocol": "rtp", "whitelistCidr": "10.9.0.0/16" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"the neighbour flow\": must be created, got {status} {code:?}: {created}"
    );

    for (form_index, form) in flow_forms().into_iter().enumerate() {
        for (row_index, candidate) in candidates(&form.names, neighbour).into_iter().enumerate() {
            let case = format!("{}, {}", form.label, candidate.case);
            let name = format!("primary-create-{form_index}-{row_index}");
            let mut body = json!({
                "name": name,
                "sourceFailoverConfig": failover(candidate.primary.as_deref()),
            });
            body[form.member] = form.sources.clone();

            let (status, code, answered) = emulator.create_flow(body).await;
            if !candidate.accepted {
                let primary = candidate.primary.clone().unwrap_or_default();
                assert_refused(&case, &primary, status, code.as_deref(), &answered);
                assert!(
                    !flow_is_listed(&emulator, &case, &name).await,
                    "case {case:?}: a refused create stores no flow, yet {name:?} is listed"
                );
                continue;
            }

            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: a priority naming a source of the flow is stored, got \
                 {status} {code:?}: {answered}"
            );
            let flow_arn = answered["flow"]["flowArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
                .to_string();
            let expected = candidate
                .primary
                .as_deref()
                .map(|primary| json!(primary))
                .unwrap_or(Value::Null);
            let stored = described_primary(&emulator, &case, &flow_arn).await;
            assert_eq!(
                stored, expected,
                "case {case:?}: the stored flow reports the primary source the create \
                 named, got {stored}"
            );

            // The stored setting is one the update path permits: putting it back unchanged
            // is accepted.
            let (status, code, updated) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "sourceFailoverConfig": failover(candidate.primary.as_deref()) })),
                )
                .await;
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the failover setting the create stored is one UpdateFlow \
                 accepts, got {status} {code:?}: {updated}"
            );
        }
    }
}

/// UpdateFlow resolves the primary source against the sources the STORED flow holds, with
/// the same verdict for the same name as the create path, and a refused request writes
/// nothing.
#[tokio::test]
async fn update_flow_resolves_the_primary_source_against_the_flow_s_stored_sources() {
    let emulator = Emulator::new();
    let neighbour = "neighbour-src";
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "primary-update-neighbour",
            "source": { "name": neighbour, "protocol": "rtp", "whitelistCidr": "10.9.0.0/16" },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case \"the neighbour flow\": must be created, got {status} {code:?}: {created}"
    );

    for (form_index, form) in flow_forms().into_iter().enumerate() {
        let name = format!("primary-update-{form_index}");
        let mut body = json!({ "name": name, "sourceFailoverConfig": failover(None) });
        body[form.member] = form.sources.clone();
        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {:?}: the flow under test must be created, got {status} {code:?}: {created}",
            form.label
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        for candidate in candidates(&form.names, neighbour) {
            let case = format!("{}, {}", form.label, candidate.case);
            let before = described_primary(&emulator, &case, &flow_arn).await;
            let (status, code, answered) = emulator
                .send(
                    "PUT",
                    &format!("/v1/flows/{flow_arn}"),
                    Some(json!({ "sourceFailoverConfig": failover(candidate.primary.as_deref()) })),
                )
                .await;
            let after = described_primary(&emulator, &case, &flow_arn).await;
            if candidate.accepted {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: a priority naming a source of the flow is accepted, got \
                     {status} {code:?}: {answered}"
                );
                // An update that carries no priority keeps the stored one (the member-wise
                // merge), so only a row that names one fixes what the flow reports.
                if let Some(primary) = candidate.primary.as_deref() {
                    assert_eq!(
                        after,
                        json!(primary),
                        "case {case:?}: the flow reports the primary source the update \
                         named, got {after}"
                    );
                }
            } else {
                let primary = candidate.primary.clone().unwrap_or_default();
                assert_refused(&case, &primary, status, code.as_deref(), &answered);
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update writes nothing, got {after}"
                );
            }
        }
    }
}

/// A source the REQUEST did not name is still a source of the flow: the name the service
/// assigns it may be the flow's primary source, and a name another flow's sources carry may
/// not.
///
/// `SetSourceRequest$Name` is optional while `Source$Name` is required, so an unnamed source
/// is named by the service — the flow's own name for its first source, `<flow name>-2` for
/// the next.
#[tokio::test]
async fn a_source_the_service_named_can_be_the_primary_source() {
    let emulator = Emulator::new();
    let case = "a pair of sources the request left unnamed";
    let name = "primary-assigned";

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "sources": [
                { "protocol": "rtp", "whitelistCidr": "10.6.0.0/16" },
                { "protocol": "rtp", "whitelistCidr": "10.7.0.0/16" },
            ],
            "sourceFailoverConfig": failover(Some(name)),
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow's own name is the name its first source was assigned, so \
         it is an acceptable primary source, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let assigned: Vec<String> = crate::flow_sources::described_sources(&emulator, case, &flow_arn)
        .await
        .iter()
        .map(|source| source["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        assigned,
        vec![name.to_string(), format!("{name}-2")],
        "case {case:?}: the service names an unnamed source after its flow, got {assigned:?}"
    );
    assert_eq!(
        described_primary(&emulator, case, &flow_arn).await,
        json!(name),
        "case {case:?}: the stored flow reports the assigned name as its primary source"
    );

    // The second assigned name is just as acceptable, through the update path.
    let (status, code, updated) = emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}"),
            Some(json!({ "sourceFailoverConfig": failover(Some(&assigned[1])) })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the flow's second assigned source name is acceptable too, got \
         {status} {code:?}: {updated}"
    );

    // And the name ANOTHER flow's sources were assigned is not: it names no source of this
    // flow, whichever path carries it.
    let other = "primary-assigned-other";
    let (status, code, answered) = emulator
        .create_flow(json!({
            "name": other,
            "source": { "protocol": "rtp", "whitelistCidr": "10.8.0.0/16" },
            "sourceFailoverConfig": failover(Some(name)),
        }))
        .await;
    assert_refused(
        "a create naming the source name another flow was assigned",
        name,
        status,
        code.as_deref(),
        &answered,
    );
    assert!(
        !flow_is_listed(&emulator, case, other).await,
        "case {case:?}: the refused create stores no flow, yet {other:?} is listed"
    );
}

/// The bridge side of the same rule: `CreateBridgeRequest$SourceFailoverConfig` and
/// `UpdateBridgeRequest$SourceFailoverConfig` target the same shapes, and a bridge's
/// sources are named too — so a priority a bridge stores names one of the bridge's own
/// sources, and anything else is refused with `BadRequestException` (400).
#[tokio::test]
async fn create_bridge_and_update_bridge_resolve_the_primary_source_against_the_bridge_s_sources() {
    let emulator = Emulator::new();
    let names = ["net-a", "net-b"];
    let sources: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({ "networkSource": {
                "multicastIp": "224.0.1.1",
                "name": name,
                "networkName": "net-1",
                "port": 5_000 + index as i64,
                "protocol": "rtp",
            }})
        })
        .collect();
    // A source name the service knows on ANOTHER bridge is still not a source of this one.
    let foreign = "net-elsewhere";

    for (row_index, candidate) in candidates(&names, foreign).into_iter().enumerate() {
        let case = format!("a ground-to-cloud bridge, {}", candidate.case);
        let name = format!("primary-bridge-{row_index}");
        let (status, code, answered) = create_bridge(
            &emulator,
            json!({
                "name": name,
                "placementArn": minted_gateway_arn(1, "gw-primary-source"),
                "ingressGatewayBridge": { "maxBitrate": 20_000_000, "maxOutputs": 2 },
                "sources": sources,
                "sourceFailoverConfig": failover(candidate.primary.as_deref()),
            }),
        )
        .await;

        if !candidate.accepted {
            let primary = candidate.primary.clone().unwrap_or_default();
            assert_refused(&case, &primary, status, code.as_deref(), &answered);
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: a priority naming a source of the bridge is stored, got {status} \
             {code:?}: {answered}"
        );
        let bridge_arn = answered["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
            .to_string();
        let (status, code, described) = describe_bridge(&emulator, &bridge_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: DescribeBridge must answer 200, got {status} {code:?}: {described}"
        );
        let expected = candidate
            .primary
            .as_deref()
            .map(|primary| json!(primary))
            .unwrap_or(Value::Null);
        let stored =
            described["bridge"]["sourceFailoverConfig"]["sourcePriority"]["primarySource"].clone();
        assert_eq!(
            stored, expected,
            "case {case:?}: the stored bridge reports the primary source the create named, \
             got {described}"
        );

        // The update path agrees on the same names, for the bridge the create stored.
        for candidate in candidates(&names, foreign) {
            let case = format!("{case} then updated with {}", candidate.case);
            let (status, code, answered) = emulator
                .send(
                    "PUT",
                    &format!("/v1/bridges/{bridge_arn}"),
                    Some(json!({
                        "sourceFailoverConfig": failover(candidate.primary.as_deref()),
                    })),
                )
                .await;
            if candidate.accepted {
                assert_eq!(
                    (status, code.as_deref()),
                    (202, None),
                    "case {case:?}: a priority naming a source of the bridge is accepted, \
                     got {status} {code:?}: {answered}"
                );
            } else {
                let primary = candidate.primary.clone().unwrap_or_default();
                assert_refused(&case, &primary, status, code.as_deref(), &answered);
            }
        }
    }
}
