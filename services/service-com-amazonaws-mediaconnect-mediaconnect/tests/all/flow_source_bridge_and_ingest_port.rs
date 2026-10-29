// SPDX-License-Identifier: CC0-1.0

//! Two rules every operation that STORES a flow source obeys, end to end over HTTP.
//!
//! 1. A `GatewayBridgeSource` names a bridge of the account — `BridgeArn` is "the ARN of
//!    the bridge feeding this flow" — so the ARN is resolved against the store before it is
//!    written: a request naming a bridge the account does not hold is refused
//!    `BadRequestException` (400) and stores nothing, and the same request naming a bridge
//!    CreateBridge minted is stored and reported by DescribeFlow.
//! 2. A source on a Zixi protocol listens on one port: "the port that the flow listens on
//!    for incoming content. If the protocol of the source is Zixi, the port must be set to
//!    2088" (`smithy_query(query="doc",shape="UpdateFlowSourceRequest",member="IngestPort")`).
//!    A request that names any other port for such a source is refused and stores nothing,
//!    and a Zixi source whose request named no port is assigned that one port.
//!
//! Both rules are asserted on EVERY path that stores a source — CreateFlow's `Source` /
//! `Sources`, AddFlowSources and UpdateFlowSource — because all three write the same two
//! members. The rows vary along what the rules generalise over: how a bridge ARN fails to
//! resolve (never minted, another Region, another account, a held ARN with a suffix, a
//! bridge DeleteBridge removed), and, for the port, the WHOLE modelled `Protocol` catalog
//! plus the ports at the edges of the port space.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::PROTOCOL_VALUES;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::bridge_lifecycle::bridge_feeding_a_flow;
use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_sources::{add_flow_sources, described_sources, failover_active};
use crate::update_flow_source::update_flow_source;

/// The port the model fixes for a Zixi source: "if the protocol of the source is Zixi, the
/// port must be set to 2088"
/// (`smithy_query(query="doc",shape="UpdateFlowSourceRequest",member="IngestPort")`).
const DOCUMENTED_ZIXI_PORT: i64 = 2088;

/// A port outside the one a Zixi source may listen on, used as the port every row requests.
const OTHER_PORT: i64 = 5101;

/// The first port the service's own assignment pool hands a source that named none
/// (`crate::state::assigned_ingest_port`), so a port it assigned is told apart from the
/// documented Zixi port.
const ASSIGNED_PORT_FLOOR: i64 = 5_000;

/// An ARN of the form this service mints a bridge in, well formed against the modelled
/// `SetGatewayBridgeSourceRequest$BridgeArn` / `UpdateGatewayBridgeSourceRequest$BridgeArn`
/// pattern (`^arn:.+:mediaconnect.+:bridge:.+$`), so the request reaches the handler
/// whatever bridge it names.
fn bridge_arn(region: &str, account: &str, id: &str, name: &str) -> String {
    format!("arn:aws:mediaconnect:{region}:{account}:bridge:{id}:{name}")
}

/// Assert `body` is this service's 400 refusal for a request whose content it would not
/// store, naming `member` and the `value` that member carried.
fn assert_refused(
    case: &str,
    member: &str,
    value: &str,
    status: u16,
    code: Option<&str>,
    body: &Value,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: a refused request keeps this service's validation envelope, got \
         {body}"
    );
    assert!(
        message.contains(member) && message.contains(value),
        "case {case:?}: the refusal names {member:?} and {value:?}, got {body}"
    );
}

/// Assert `body` is that same refusal for a port the Zixi rule forbids: it names the
/// member, the OFFENDING port the request carried, and the one port such a source listens
/// on.
fn assert_port_refused(
    case: &str,
    member: &str,
    requested: i64,
    status: u16,
    code: Option<&str>,
    body: &Value,
) {
    assert_refused(
        case,
        member,
        &requested.to_string(),
        status,
        code,
        body,
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&DOCUMENTED_ZIXI_PORT.to_string()),
        "case {case:?}: the refusal states the port a Zixi source listens on \
         ({DOCUMENTED_ZIXI_PORT}), got {body}"
    );
}

/// Create the flow the rows work on: one plain source, source failover ACTIVE so one more
/// source can be added, and the NDI-capable size the `ndi-speed-hq` rows need ("set this
/// optional parameter to LARGE if you want to enable NDI sources or outputs on the flow",
/// `CreateFlowRequest$FlowSize`). Answers `(FlowArn, SourceArn)`.
async fn fixture_flow(emulator: &Emulator, case: &str, name: &str) -> (String, String) {
    let flow_arn = crate::flow_sources::create_flow_from(
        emulator,
        case,
        json!({
            "name": name,
            "flowSize": "LARGE",
            "sourceFailoverConfig": failover_active(),
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        }),
    )
    .await;
    let source_arn = described_sources(emulator, case, &flow_arn).await[0]["sourceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the fixture source carries an ARN"))
        .to_string();
    (flow_arn, source_arn)
}

/// One row of the bridge-reference table: how the ARN fails to resolve.
struct UnresolvableBridge {
    case: String,
    arn: String,
}

/// Every path that stores a `GatewayBridgeSource` resolves its `BridgeArn` against the
/// bridges the account holds: a request naming one it does not hold is refused and stores
/// nothing, whichever operation carries it.
#[tokio::test]
async fn every_path_that_stores_a_flow_source_resolves_its_bridge_arn() {
    let emulator = Emulator::new();
    let held = bridge_feeding_a_flow(&emulator, "br-held").await;

    // A bridge the account held and DeleteBridge removed: the reference is resolved against
    // what the store holds NOW, not against every ARN this service ever minted.
    let removed = bridge_feeding_a_flow(&emulator, "br-removed").await;
    let (status, code, deleted) = emulator
        .send("DELETE", &format!("/v1/bridges/{removed}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "setup: the bridge under test must be deleted, got {status} {code:?}: {deleted}"
    );

    let rows = vec![
        UnresolvableBridge {
            case: "a bridge ARN this service never minted".to_string(),
            arn: bridge_arn(REGION, "111122223333", "1-0000dead", "never-minted"),
        },
        UnresolvableBridge {
            case: "a bridge ARN of another Region".to_string(),
            arn: bridge_arn("eu-west-1", "111122223333", "1-0000dead", "elsewhere"),
        },
        UnresolvableBridge {
            case: "a bridge ARN of another account".to_string(),
            arn: bridge_arn(REGION, "999988887777", "1-0000dead", "other-account"),
        },
        UnresolvableBridge {
            // ARNs are matched exactly, so the ARN of a bridge the account HOLDS with one
            // more character names nothing.
            case: "a held bridge's ARN with a suffix".to_string(),
            arn: format!("{held}-2"),
        },
        UnresolvableBridge {
            case: "the ARN of a bridge DeleteBridge removed".to_string(),
            arn: removed.clone(),
        },
    ];

    let (flow_arn, source_arn) =
        fixture_flow(&emulator, "bridge references", "flow-bridge-reference").await;
    let (_, _, before_listing) = emulator.list_flows("").await;
    let flows_before = before_listing["flows"].as_array().cloned().unwrap_or_default().len();

    for UnresolvableBridge { case, arn } in &rows {
        let before = described_sources(&emulator, case, &flow_arn).await;

        // CreateFlow's own `Source`.
        let (status, code, created) = emulator
            .create_flow(json!({
                "name": "flow-bridge-refused",
                "source": { "name": "src", "gatewayBridgeSource": { "bridgeArn": arn } },
            }))
            .await;
        assert_refused(
            &format!("{case}, on CreateFlow"),
            "Sources[0].GatewayBridgeSource.BridgeArn",
            arn,
            status,
            code.as_deref(),
            &created,
        );
        let (_, _, listing) = emulator.list_flows("").await;
        assert_eq!(
            listing["flows"].as_array().cloned().unwrap_or_default().len(),
            flows_before,
            "case {case:?}: a refused create stores no flow, got {listing}"
        );

        // AddFlowSources.
        let (status, code, added) = add_flow_sources(
            &emulator,
            &flow_arn,
            json!([{ "name": "added", "gatewayBridgeSource": { "bridgeArn": arn } }]),
        )
        .await;
        assert_refused(
            &format!("{case}, on AddFlowSources"),
            "Sources[0].GatewayBridgeSource.BridgeArn",
            arn,
            status,
            code.as_deref(),
            &added,
        );

        // UpdateFlowSource, whose request shape carries the member unnested.
        let (status, code, updated) = update_flow_source(
            &emulator,
            &flow_arn,
            &source_arn,
            json!({ "gatewayBridgeSource": { "bridgeArn": arn } }),
        )
        .await;
        assert_refused(
            &format!("{case}, on UpdateFlowSource"),
            "GatewayBridgeSource.BridgeArn",
            arn,
            status,
            code.as_deref(),
            &updated,
        );

        let after = described_sources(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request leaves the flow's sources as they were, got \
             {after:?}"
        );
    }

    // The same three requests, each naming the bridge the account HOLDS: stored, and read
    // back through DescribeFlow.
    let case = "a bridge this account holds";
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "flow-bridge-created",
            "source": { "name": "src", "gatewayBridgeSource": { "bridgeArn": held } },
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateFlow stores a source fed by a held bridge, got {status} \
         {code:?}: {created}"
    );
    let created_flow = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let stored = described_sources(&emulator, case, &created_flow).await;
    assert_eq!(
        stored[0]["gatewayBridgeSource"]["bridgeArn"].as_str(),
        Some(held.as_str()),
        "case {case:?}: DescribeFlow reports the bridge the create stored, got {stored:?}"
    );

    let (status, code, added) = add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "added", "gatewayBridgeSource": { "bridgeArn": held } }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: AddFlowSources appends a source fed by a held bridge, got {status} \
         {code:?}: {added}"
    );
    let sources = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        sources
            .last()
            .map(|source| source["gatewayBridgeSource"]["bridgeArn"].clone())
            .unwrap_or(Value::Null),
        json!(held),
        "case {case:?}: DescribeFlow reports the bridge the add stored, got {sources:?}"
    );

    let (status, code, updated) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({ "gatewayBridgeSource": { "bridgeArn": held } }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: UpdateFlowSource stores a held bridge, got {status} {code:?}: \
         {updated}"
    );
    let sources = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        sources[0]["gatewayBridgeSource"]["bridgeArn"].as_str(),
        Some(held.as_str()),
        "case {case:?}: DescribeFlow reports the bridge the update stored, got {sources:?}"
    );
}

/// The port rule over the WHOLE modelled `Protocol` catalog
/// (`service_..::ops::types::PROTOCOL_VALUES`, the `Protocol` enum's values): a requested
/// port other than the documented one is refused for a Zixi source on every path that
/// stores a source, and kept for a source on any other protocol.
#[tokio::test]
async fn a_port_other_than_the_zixi_port_is_refused_for_a_zixi_source_on_every_path() {
    let emulator = Emulator::new();
    assert_eq!(
        i64::from(mc::ZIXI_INGEST_PORT),
        DOCUMENTED_ZIXI_PORT,
        "the port this crate fixes for a Zixi source is the documented one, got {}",
        mc::ZIXI_INGEST_PORT
    );

    for protocol in PROTOCOL_VALUES {
        // Which protocols the sentence governs is read off the protocol's own name, not off
        // the crate's list, and the two must agree.
        let zixi = protocol.starts_with("zixi-");
        assert_eq!(
            mc::is_zixi_protocol(protocol),
            zixi,
            "protocol {protocol:?}: a Zixi protocol is one the `Protocol` enum names zixi-*"
        );

        let (flow_arn, source_arn) = fixture_flow(
            &emulator,
            protocol,
            &format!("flow-port-{}", protocol.replace('-', "")),
        )
        .await;
        let before = described_sources(&emulator, protocol, &flow_arn).await;

        // CreateFlow.
        let case = format!("a {protocol:?} source created on port {OTHER_PORT}");
        let (status, code, created) = emulator
            .create_flow(json!({
                "name": "flow-port-created",
                "flowSize": "LARGE",
                "source": {
                    "name": "src",
                    "protocol": protocol,
                    "whitelistCidr": "10.0.0.0/16",
                    "ingestPort": OTHER_PORT
                },
            }))
            .await;
        if zixi {
            assert_port_refused(
                &case,
                "Sources[0].IngestPort",
                OTHER_PORT,
                status,
                code.as_deref(),
                &created,
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: a source outside the Zixi protocols keeps the port it asked \
                 for, got {status} {code:?}: {created}"
            );
            assert_eq!(
                created["flow"]["source"]["ingestPort"].as_i64(),
                Some(OTHER_PORT),
                "case {case:?}: the stored source listens on the requested port, got {created}"
            );
        }

        // AddFlowSources.
        let case = format!("a {protocol:?} source added on port {OTHER_PORT}");
        let (status, code, added) = add_flow_sources(
            &emulator,
            &flow_arn,
            json!([{
                "name": "added",
                "protocol": protocol,
                "whitelistCidr": "10.1.0.0/16",
                "ingestPort": OTHER_PORT
            }]),
        )
        .await;
        if zixi {
            assert_port_refused(
                &case,
                "Sources[0].IngestPort",
                OTHER_PORT,
                status,
                code.as_deref(),
                &added,
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the add keeps the port it asked for, got {status} {code:?}: \
                 {added}"
            );
            assert_eq!(
                added["sources"][0]["ingestPort"].as_i64(),
                Some(OTHER_PORT),
                "case {case:?}: the appended source listens on the requested port, got {added}"
            );
        }

        // UpdateFlowSource, which carries the protocol and the port in one request.
        let case = format!("a source moved onto {protocol:?} with port {OTHER_PORT}");
        let (status, code, updated) = update_flow_source(
            &emulator,
            &flow_arn,
            &source_arn,
            json!({ "protocol": protocol, "ingestPort": OTHER_PORT }),
        )
        .await;
        if zixi {
            assert_port_refused(
                &case,
                "IngestPort",
                OTHER_PORT,
                status,
                code.as_deref(),
                &updated,
            );
            let after = described_sources(&emulator, &case, &flow_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update writes nothing, got {after:?}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (202, None),
                "case {case:?}: the update keeps the port it asked for, got {status} {code:?}: \
                 {updated}"
            );
            assert_eq!(
                (
                    updated["source"]["ingestPort"].as_i64(),
                    updated["source"]["transport"]["protocol"].as_str()
                ),
                (Some(OTHER_PORT), Some(*protocol)),
                "case {case:?}: the stored source listens on the requested port, got {updated}"
            );
        }
    }
}

/// What the rule says a stored source then listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedPort {
    /// The one port the model fixes for the protocol.
    Fixed(i64),
    /// Any port the service's own assignment pool hands out.
    FromPool,
    /// No ingest endpoint at all.
    None,
}

/// One row of the assignment table: the source a request stores, and the port the rule says
/// it then listens on.
struct PortCase {
    case: &'static str,
    source: Value,
    expected: ExpectedPort,
}

/// The port a source listens on when its request named none, or named the documented one:
/// a Zixi source listens on the documented port whichever call put it on that protocol, a
/// source on any other protocol is assigned one from the service's pool, and a source
/// reached through an entitlement listens on no ingest endpoint of its own at all.
#[tokio::test]
async fn a_zixi_source_listens_on_the_documented_port_however_it_got_there() {
    let emulator = Emulator::new();

    let rows = vec![
        PortCase {
            case: "a zixi-push source whose request named no port",
            source: json!({ "name": "src", "protocol": "zixi-push", "whitelistCidr": "10.0.0.0/16" }),
            expected: ExpectedPort::Fixed(DOCUMENTED_ZIXI_PORT),
        },
        PortCase {
            case: "a zixi-pull source that named the documented port",
            source: json!({
                "name": "src",
                "protocol": "zixi-pull",
                "whitelistCidr": "10.0.0.0/16",
                "ingestPort": DOCUMENTED_ZIXI_PORT
            }),
            expected: ExpectedPort::Fixed(DOCUMENTED_ZIXI_PORT),
        },
        PortCase {
            case: "an rtp source whose request named no port",
            source: json!({ "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
            expected: ExpectedPort::FromPool,
        },
        PortCase {
            // "A source reached through another account's entitlement is fed by that
            // account's flow", so this service assigns it no ingest endpoint — and the Zixi
            // rule gives it no port either.
            case: "an entitled zixi-push source",
            source: json!({
                "name": "src",
                "protocol": "zixi-push",
                "entitlementArn":
                    "arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared"
            }),
            expected: ExpectedPort::None,
        },
    ];

    for (index, PortCase { case, source, expected }) in rows.into_iter().enumerate() {
        let flow_arn = crate::flow_sources::create_flow_from(
            &emulator,
            case,
            json!({ "name": format!("flow-assigned-port-{index}"), "source": source }),
        )
        .await;
        let stored = described_sources(&emulator, case, &flow_arn).await;
        let port = stored[0]["ingestPort"].as_i64();
        match expected {
            ExpectedPort::Fixed(fixed) => assert_eq!(
                port,
                Some(fixed),
                "case {case:?}: the source listens on port {fixed}, got {stored:?}"
            ),
            ExpectedPort::FromPool => assert!(
                port.is_some_and(|port| (ASSIGNED_PORT_FLOOR..=65_535).contains(&port)),
                "case {case:?}: the source is assigned a port from the service's pool, got \
                 {stored:?}"
            ),
            ExpectedPort::None => assert!(
                stored[0]["ingestPort"].is_null(),
                "case {case:?}: an entitled source listens on no ingest endpoint, got \
                 {stored:?}"
            ),
        }
    }

    // The update path agrees: a source the request moves onto a Zixi protocol listens on
    // the documented port from then on, even though the request named no port.
    let case = "a source moved onto zixi-pull without naming a port";
    let (flow_arn, source_arn) = fixture_flow(&emulator, case, "flow-moved-onto-zixi").await;
    let (status, code, before) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({ "ingestPort": OTHER_PORT }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref(), before["source"]["ingestPort"].as_i64()),
        (202, None, Some(OTHER_PORT)),
        "setup {case:?}: the rtp source takes the port it asked for, got {status} {code:?}: \
         {before}"
    );
    let (status, code, updated) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({ "protocol": "zixi-pull" }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202 with no error, got {status} {code:?}: {updated}"
    );
    assert_eq!(
        (
            updated["source"]["ingestPort"].as_i64(),
            updated["source"]["transport"]["protocol"].as_str()
        ),
        (Some(DOCUMENTED_ZIXI_PORT), Some("zixi-pull")),
        "case {case:?}: the source now listens on the documented Zixi port, got {updated}"
    );
    let stored = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        stored[0], updated["source"],
        "case {case:?}: DescribeFlow reports what the update answered, got {stored:?}"
    );
}
