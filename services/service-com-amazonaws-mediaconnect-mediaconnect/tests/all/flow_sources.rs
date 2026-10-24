// SPDX-License-Identifier: CC0-1.0

//! AddFlowSources and RemoveFlowSource, end to end over HTTP: every requested source is
//! materialised into a record of the flow it was added to, DescribeFlow reports what the
//! add appended, and a remove drops exactly the source its ARN addresses.
//!
//! The cases drive the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};

/// The ARN of a flow in this Region that this service never minted — well formed enough
/// to pass the modelled `FlowArn` label pattern, so the request reaches the handler.
pub(crate) fn unknown_flow_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-minted")
}

/// The ARN of a source this service never minted, well formed enough to pass the
/// modelled `SourceArn` label pattern
/// (`smithy_query(query="operation",operation="RemoveFlowSource")`:
/// `^arn:.+:mediaconnect.+:source:.+$`), so the request reaches the handler.
pub(crate) fn unknown_source_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:source:1-0000dead:never-minted")
}

/// `POST /v1/flows/{FlowArn}/source` — the AddFlowSources binding
/// (`smithy_query(query="operation",operation="AddFlowSources")`).
pub(crate) async fn add_flow_sources(
    emulator: &Emulator,
    flow_arn: &str,
    sources: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/source"),
            Some(json!({ "sources": sources })),
        )
        .await
}

/// `DELETE /v1/flows/{FlowArn}/source/{SourceArn}` — the RemoveFlowSource binding
/// (`smithy_query(query="operation",operation="RemoveFlowSource")`).
pub(crate) async fn remove_flow_source(
    emulator: &Emulator,
    flow_arn: &str,
    source_arn: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/source/{source_arn}"),
            None,
        )
        .await
}

/// The source failover config a flow needs to hold a PAIR of sources:
/// `FailoverConfig$State` ENABLED — "if the state is active, the flow can have one or two
/// sources" (`smithy_query(query="doc",shape="FailoverConfig",member="State")`).
pub(crate) fn failover_active() -> Value {
    json!({ "state": "ENABLED" })
}

/// Create the flow a case works on, with `initial` sources already on it and `failover` as
/// its `SourceFailoverConfig`, and return its ARN.
pub(crate) async fn create_flow_with_sources_and_failover(
    emulator: &Emulator,
    case: &str,
    name: &str,
    initial: Value,
    failover: Option<Value>,
) -> String {
    let mut body = json!({ "name": name, "sources": initial });
    if let Some(failover) = failover {
        body["sourceFailoverConfig"] = failover;
    }
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create the flow a case works on, with `initial` sources already on it, and return its
/// ARN.
///
/// A flow holding more than one source needs source failover ACTIVE — "if the state is
/// inactive, the flow can have only one source" (`FailoverConfig$State`) — so a case that
/// asks for a pair gets a flow whose failover state is ENABLED.
pub(crate) async fn create_flow_with_sources(
    emulator: &Emulator,
    case: &str,
    name: &str,
    initial: Value,
) -> String {
    let needs_failover = initial.as_array().is_some_and(|sources| sources.len() > 1);
    create_flow_with_sources_and_failover(
        emulator,
        case,
        name,
        initial,
        needs_failover.then(failover_active),
    )
    .await
}

/// The flow's stored sources, as DescribeFlow reports them.
pub(crate) async fn described_sources(emulator: &Emulator, case: &str, flow_arn: &str) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every `SourceArn` the service mints must satisfy the pattern the model puts on the
/// `SourceArn` label of the operations that address one source
/// (`smithy_query(query="operation",operation="RemoveFlowSource")`:
/// `^arn:.+:mediaconnect.+:source:.+$`), and carry the source's own name as its last
/// segment, the way a flow ARN carries the flow's.
fn assert_minted_source_arn(case: &str, arn: &str, name: &str) {
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: a source ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "source"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert!(
        !segments[6].is_empty(),
        "case {case:?}: the source carries an id of its own, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the source's name, got {arn:?}"
    );
}

/// The expected name of a generated source, with `{flow}` standing for the flow's own
/// name: a generated name is derived from it, so a row states the shape rather than the
/// name of the flow the case happens to create.
fn expand(expected: &str, flow_name: &str) -> String {
    expected.replace("{flow}", flow_name)
}

/// One row of the AddFlowSources table: the sources the flow is created with, the sources
/// one AddFlowSources call then adds, the names the class rule says those added sources
/// take, and the `Transport.MaxBitrate` each of them must report.
struct AddCase {
    case: &'static str,
    initial: Value,
    add: Value,
    expected_names: &'static [&'static str],
    /// One entry per added source: the bitrate the source must report, or `None` where
    /// it reports none. `SetSourceRequest$MaxBitrate` is "the smoothing max bitrate (in
    /// bps) for RIST, RTP, and RTP-FEC streams", so a request that names one keeps it and
    /// a source on one of those protocols that names none gets the service's assignment.
    expected_max_bitrate: &'static [Option<i64>],
}

/// The rows differ along what the rule generalises over: whether the added source names
/// itself or leaves the service to, which protocol and transport members it carries (or
/// none at all), whether it is owned or reached through an entitlement, and whether it
/// asks for an ingest port, a decryption config or router integration.
///
/// Every row starts from a flow with ONE source and adds ONE more: "if the state is
/// active, the flow can have one or two sources" (`FailoverConfig$State`), so the pair is
/// the largest flow an add can build. The bound itself is the subject of
/// `crate::flow_source_failover_bound`.
fn add_cases() -> Vec<AddCase> {
    vec![
        AddCase {
            case: "one named rtp source onto a flow with one source",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{ "name": "backup", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" }]),
            expected_names: &["backup"],
            expected_max_bitrate: &[Some(160_000_000)],
        },
        AddCase {
            case: "an unnamed udp source takes the flow's next generated name",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{ "protocol": "udp", "whitelistCidr": "10.3.0.0/16" }]),
            expected_names: &["{flow}-2"],
            expected_max_bitrate: &[None],
        },
        AddCase {
            case: "a named zixi-push source keeps its latency, stream id and description",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{
                "name": "zixi",
                "protocol": "zixi-push",
                "whitelistCidr": "10.4.0.0/16",
                "maxLatency": 2000,
                "streamId": "s-7",
                "description": "the pushed stream"
            }]),
            expected_names: &["zixi"],
            expected_max_bitrate: &[None],
        },
        AddCase {
            case: "an unnamed rtp-fec source gets the service's smoothing bitrate",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{ "protocol": "rtp-fec", "whitelistCidr": "10.5.0.0/16" }]),
            expected_names: &["{flow}-2"],
            expected_max_bitrate: &[Some(160_000_000)],
        },
        AddCase {
            case: "an entitled source carries no ingest endpoint and no transport",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{
                "name": "entitled",
                "entitlementArn":
                    "arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared",
                "description": "another account's flow feeds this one"
            }]),
            expected_names: &["entitled"],
            expected_max_bitrate: &[None],
        },
        AddCase {
            case: "an explicit ingest port and a requested max bitrate are kept",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{
                "name": "explicit",
                "protocol": "rist",
                "whitelistCidr": "10.6.0.0/16",
                "ingestPort": 9100,
                "maxBitrate": 7_000_000,
                "maxLatency": 1500,
                "minLatency": 100,
                "maxSyncBuffer": 1000,
                "senderIpAddress": "198.51.100.9",
                "senderControlPort": 9101,
                "streamId": "s-1"
            }]),
            expected_names: &["explicit"],
            expected_max_bitrate: &[Some(7_000_000)],
        },
        AddCase {
            case: "decryption, a VPC interface and router integration are stored as requested",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
            add: json!([{
                "name": "encrypted",
                "protocol": "srt-listener",
                "minLatency": 120,
                "vpcInterfaceName": "vpc-1",
                "routerIntegrationState": "ENABLED",
                "decryption": {
                    "algorithm": "aes256",
                    "keyType": "static-key",
                    "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                    "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-abc"
                }
            }]),
            expected_names: &["encrypted"],
            expected_max_bitrate: &[None],
        },
        AddCase {
            case: "the srt-caller listener members fold into the transport",
            initial: json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.7.0.0/16" }]),
            add: json!([{
                "name": "caller",
                "protocol": "srt-caller",
                "sourceListenerAddress": "198.51.100.20",
                "sourceListenerPort": 9200,
                "minLatency": 150
            }]),
            expected_names: &["caller"],
            expected_max_bitrate: &[None],
        },
    ]
}

/// AddFlowSources materialises each requested source into a stored record with its own
/// ARN and a service-assigned ingest endpoint, appends it to the flow it names, and
/// answers with those records — the same bytes DescribeFlow then reports.
#[tokio::test]
async fn add_flow_sources_persists_the_sources_it_returns() {
    let emulator = Emulator::new();

    for (index, case_row) in add_cases().into_iter().enumerate() {
        let AddCase {
            case,
            initial,
            add,
            expected_names,
            expected_max_bitrate,
        } = case_row;
        let flow_name = format!("flow-add-sources-{index}");
        // The flow carries source failover ACTIVE, because the add gives it a second
        // source: "if the state is active, the flow can have one or two sources"
        // (`FailoverConfig$State`).
        let flow_arn = create_flow_with_sources_and_failover(
            &emulator,
            case,
            &flow_name,
            initial.clone(),
            Some(failover_active()),
        )
        .await;
        let before = described_sources(&emulator, case, &flow_arn).await;

        let (status, code, body) = add_flow_sources(&emulator, &flow_arn, add.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the sources were added to, got {:?}",
            body["flowArn"]
        );

        let requested = add.as_array().cloned().unwrap_or_default();
        let added = body["sources"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            added.len(),
            requested.len(),
            "case {case:?}: one Source per requested source, got {added:?}"
        );

        for (position, (source, request)) in added.iter().zip(requested.iter()).enumerate() {
            let name = source["name"].as_str().unwrap_or_default();
            assert_eq!(
                name,
                expand(expected_names[position], &flow_name),
                "case {case:?}: source {position} name, got {source:?}"
            );
            assert_minted_source_arn(case, source["sourceArn"].as_str().unwrap_or_default(), name);

            // The members `SetSourceRequest` and `Source` share are kept verbatim.
            for member in [
                "description",
                "whitelistCidr",
                "entitlementArn",
                "vpcInterfaceName",
                "senderIpAddress",
                "senderControlPort",
                "decryption",
            ] {
                assert_eq!(
                    source[member], request[member],
                    "case {case:?}: source {position} {member}, got {source:?}"
                );
            }

            // `Source$RouterIntegrationState` is reported on every source: the state the
            // request asked for, and the disabled one when it asked for none.
            assert_eq!(
                source["routerIntegrationState"].as_str(),
                Some(request["routerIntegrationState"].as_str().unwrap_or("DISABLED")),
                "case {case:?}: source {position} routerIntegrationState, got {source:?}"
            );

            // A source reached through another account's entitlement is fed by that
            // account's flow, so it listens on no ingest endpoint of its own; an owned
            // source listens on the address the service assigns it, and on the port the
            // request named or the one the service assigned.
            if request["entitlementArn"].is_string() {
                assert!(
                    source["ingestIp"].is_null() && source["ingestPort"].is_null(),
                    "case {case:?}: source {position} is entitled and has no ingest endpoint, \
                     got {source:?}"
                );
            } else {
                assert!(
                    source["ingestIp"].is_string(),
                    "case {case:?}: source {position} listens on an assigned address, got \
                     {source:?}"
                );
                match request["ingestPort"].as_i64() {
                    Some(port) => assert_eq!(
                        source["ingestPort"].as_i64(),
                        Some(port),
                        "case {case:?}: source {position} keeps the port it asked for, got \
                         {source:?}"
                    ),
                    None => assert!(
                        source["ingestPort"].is_number(),
                        "case {case:?}: source {position} is assigned a port, got {source:?}"
                    ),
                }
            }

            // The transport members of the request fold into `Source.Transport`, and a
            // source that names no protocol has no transport at all
            // (`Transport.Protocol` is required).
            match request["protocol"].as_str() {
                Some(protocol) => {
                    assert_eq!(
                        source["transport"]["protocol"].as_str(),
                        Some(protocol),
                        "case {case:?}: source {position} transport protocol, got {source:?}"
                    );
                    for member in [
                        "maxLatency",
                        "maxSyncBuffer",
                        "minLatency",
                        "senderControlPort",
                        "senderIpAddress",
                        "sourceListenerAddress",
                        "sourceListenerPort",
                        "streamId",
                    ] {
                        assert_eq!(
                            source["transport"][member], request[member],
                            "case {case:?}: source {position} transport {member}, got {source:?}"
                        );
                    }
                    assert_eq!(
                        source["transport"]["maxBitrate"].as_i64(),
                        expected_max_bitrate[position],
                        "case {case:?}: source {position} transport maxBitrate, got {source:?}"
                    );
                }
                None => assert!(
                    source["transport"].is_null(),
                    "case {case:?}: source {position} has no transport, got {source:?}"
                ),
            }
        }

        // The round trip: the flow now holds the sources it held before, in order,
        // followed by the ones this call added — byte for byte what the add answered.
        let after = described_sources(&emulator, case, &flow_arn).await;
        let expected: Vec<Value> = before.iter().chain(added.iter()).cloned().collect();
        assert_eq!(
            after, expected,
            "case {case:?}: DescribeFlow reports the previous sources then the added ones, \
             got {after:?}"
        );

        // `Flow.Source` is the flow's first source, which an add never changes.
        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["source"], after[0],
            "case {case:?}: flow.source stays the flow's first source, got {:?}",
            described["flow"]["source"]
        );
        // Adding a source does not move the flow's status.
        assert_eq!(
            described["flow"]["status"].as_str(),
            Some("STANDBY"),
            "case {case:?}: the flow keeps the status it had, got {:?}",
            described["flow"]["status"]
        );
    }
}

/// One row of the refusal table: a request AddFlowSources must refuse, and the modelled
/// error it must refuse it with.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    sources: Value,
    status: u16,
    code: &'static str,
}

/// A request the service will not satisfy is refused with the operation's own modelled
/// error, and stores nothing: the flow still holds exactly the sources it held.
#[tokio::test]
async fn add_flow_sources_refuses_what_it_cannot_add() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_sources(
        &emulator,
        "refusals",
        "flow-source-refusals",
        json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
    )
    .await;
    let before = described_sources(&emulator, "refusals", &flow_arn).await;

    let one_source = json!([{ "name": "fresh", "protocol": "udp", "whitelistCidr": "10.8.0.0/16" }]);
    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            sources: one_source.clone(),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "a request that adds no source at all",
            flow_arn: flow_arn.clone(),
            sources: json!([]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // "If the state is inactive, the flow can have only one source"
            // (`FailoverConfig$State`): this flow carries no `SourceFailoverConfig` at
            // all, so a second source is one too many. The whole rule is the subject of
            // `crate::flow_source_failover_bound`.
            case: "a second source on a flow whose source failover is inactive",
            flow_arn: flow_arn.clone(),
            sources: one_source.clone(),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target,
        sources,
        status: expected_status,
        code: expected_code,
    } in rows
    {
        let (status, code, body) = add_flow_sources(&emulator, &target, sources).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        let after = described_sources(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request stores nothing, got {after:?}"
        );
    }
}

/// `count` source requests, named `src-<n>` from `first`, so a case can fill a flow to a
/// known number of sources.
fn numbered_sources(first: usize, count: usize) -> Value {
    (first..first + count)
        .map(|index| {
            json!({
                "name": format!("src-{index}"),
                "protocol": "udp",
                "whitelistCidr": "10.9.0.0/16"
            })
        })
        .collect()
}

/// A source name the service generates is the next free one in the flow's own sequence —
/// across calls, and around names a caller chose for itself — so no two sources of one
/// flow are generated with the same name.
#[tokio::test]
async fn generated_source_names_stay_unique_across_calls() {
    let emulator = Emulator::new();
    let case = "a generated name skips the one already taken";
    let flow_name = "flow-generated-sources";
    // The flow's first source is named `<flow>-2` by the request, which is exactly the
    // name the service would generate for the source at position 1 — so the add has to
    // skip it. Source failover is ACTIVE, because the flow ends up with a pair.
    let flow_arn = create_flow_with_sources_and_failover(
        &emulator,
        case,
        flow_name,
        json!([{ "name": "flow-generated-sources-2", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" }]),
        Some(failover_active()),
    )
    .await;

    let (status, code, body) =
        add_flow_sources(&emulator, &flow_arn, json!([{ "protocol": "udp" }])).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
    );
    assert_eq!(
        body["sources"][0]["name"].as_str(),
        Some("flow-generated-sources-3"),
        "case {case:?}: the generated name is the first free one, got {body}"
    );

    let on_the_flow = described_sources(&emulator, case, &flow_arn).await;
    let stored: Vec<&str> = on_the_flow
        .iter()
        .map(|source| source["name"].as_str().unwrap_or_default())
        .collect();
    let mut unique = stored.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        stored.len(),
        "case {case:?}: every source name on the flow is unique, got {stored:?}"
    );
}

/// RemoveFlowSource drops exactly the source its `SourceArn` addresses, whichever
/// position that source holds, leaves the flow's other sources in order, and answers with
/// the flow and the source it removed.
#[tokio::test]
async fn remove_flow_source_drops_the_source_it_names() {
    let emulator = Emulator::new();

    // The rows differ along which position the removed source holds: the first (so
    // `Flow.Source` moves) and the last. A flow holds at most the failover pair
    // ("if the state is active, the flow can have one or two sources",
    // `FailoverConfig$State`), so those two positions are every position there is.
    for (case, removed_position) in [
        ("the first source of the flow", 0usize),
        ("the last source of the flow", 1),
    ] {
        let flow_arn = create_flow_with_sources(
            &emulator,
            case,
            &format!("flow-remove-source-{removed_position}"),
            numbered_sources(0, 2),
        )
        .await;
        let before = described_sources(&emulator, case, &flow_arn).await;
        let target = before[removed_position]["sourceArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, code, body) = remove_flow_source(&emulator, &flow_arn, &target).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {body}"
        );
        assert_eq!(
            (body["flowArn"].as_str(), body["sourceArn"].as_str()),
            (Some(flow_arn.as_str()), Some(target.as_str())),
            "case {case:?}: the response names the flow and the source removed, got {body}"
        );

        let after = described_sources(&emulator, case, &flow_arn).await;
        let expected: Vec<Value> = before
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != removed_position)
            .map(|(_, source)| source.clone())
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: the flow keeps its other sources, in order, got {after:?}"
        );

        // `Flow.Source` is whichever source is now first.
        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["source"], after[0],
            "case {case:?}: flow.source is the flow's first remaining source, got {:?}",
            described["flow"]["source"]
        );
    }
}

/// A remove the service will not satisfy is refused with the operation's own modelled
/// error and drops nothing: an ARN no source of the flow carries resolves no source
/// (404), and a flow's last source cannot be removed at all (400) — "this request can be
/// made only if there is more than one source on the flow".
#[tokio::test]
async fn remove_flow_source_refuses_what_it_cannot_remove() {
    let emulator = Emulator::new();
    let case = "a flow with two sources";
    let flow_arn = create_flow_with_sources(
        &emulator,
        case,
        "flow-remove-source-refusals",
        numbered_sources(0, 2),
    )
    .await;
    let before = described_sources(&emulator, case, &flow_arn).await;
    let held = before[0]["sourceArn"].as_str().unwrap_or_default().to_string();
    let other = before[1]["sourceArn"].as_str().unwrap_or_default().to_string();

    for (case, target_flow, target_source, expected_status, expected_code) in [
        (
            "a flow ARN this service never minted",
            unknown_flow_arn(),
            held.clone(),
            404,
            "NotFoundException",
        ),
        (
            "a source ARN no source of the flow carries",
            flow_arn.clone(),
            unknown_source_arn(),
            404,
            "NotFoundException",
        ),
    ] {
        let (status, code, body) =
            remove_flow_source(&emulator, &target_flow, &target_source).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        let after = described_sources(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused remove drops nothing, got {after:?}"
        );
    }

    // Down to one source: the remove that empties the flow is refused, and the source it
    // named is still there.
    let case = "the flow's last source";
    let (status, code, body) = remove_flow_source(&emulator, &flow_arn, &other).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the second source is removable, got {status} {code:?}: {body}"
    );
    let (status, code, body) = remove_flow_source(&emulator, &flow_arn, &held).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
    );
    let after = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: the flow still holds its one source, got {after:?}"
    );
    assert_eq!(
        after[0]["sourceArn"].as_str(),
        Some(held.as_str()),
        "case {case:?}: and it is the source the refused remove named, got {after:?}"
    );
}

/// Add and remove are the two halves of one rule: a source added by one call is
/// addressable by the next, and the slot it freed is reused by a later generated name
/// rather than minting a duplicate.
#[tokio::test]
async fn sources_added_then_removed_leave_the_flow_consistent() {
    let emulator = Emulator::new();
    let case = "add, remove, add again";
    let flow_name = "flow-source-round-trip";
    let flow_arn = create_flow_with_sources_and_failover(
        &emulator,
        case,
        flow_name,
        json!([{ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" }]),
        Some(failover_active()),
    )
    .await;

    let (status, _, added) = add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "second", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" }]),
    )
    .await;
    assert_eq!(status, 201, "case {case:?}: the add is accepted, got {added}");
    let second = added["sources"][0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, removed) = remove_flow_source(&emulator, &flow_arn, &second).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the source the add minted is addressable, got {status} {code:?}: \
         {removed}"
    );
    let sources = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        sources.len(),
        1,
        "case {case:?}: the flow is back to its one source, got {sources:?}"
    );

    // The next unnamed source lands at position 1 again, so it takes the free name there.
    let (status, code, again) =
        add_flow_sources(&emulator, &flow_arn, json!([{ "protocol": "udp" }])).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: expected 201 with no error, got {status} {code:?}: {again}"
    );
    assert_eq!(
        again["sources"][0]["name"].as_str(),
        Some(format!("{flow_name}-2").as_str()),
        "case {case:?}: the generated name is the free one at that position, got {again}"
    );
    assert_eq!(
        described_sources(&emulator, case, &flow_arn).await.len(),
        2,
        "case {case:?}: the flow holds both sources"
    );
}
