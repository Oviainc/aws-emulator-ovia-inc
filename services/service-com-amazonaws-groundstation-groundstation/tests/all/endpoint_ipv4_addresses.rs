// SPDX-License-Identifier: CC0-1.0

//! Every value this service holds to the model's `IpV4Address` is parsed as an address.
//!
//! The RULE, one level above any single operation: a member whose target is `IpV4Address`
//! carries a dotted-quad IPv4 address, and the shape's own pattern only holds a value to
//! four dot-separated runs of up to three digits
//! (`smithy_query(query="shape", shape="IpV4Address")`), so the service parses it and
//! refuses one whose octets run past the published 0..255 bound with
//! `InvalidParameterException` (HTTP 431), naming the member at fault and storing nothing.
//!
//! Asserted over a table rather than one recorded call:
//!
//! * the value spans addresses at each bound of the shape's own length, addresses inside
//!   it, and values that satisfy its pattern and name no address — one octet past the
//!   bound, in each position, with one, two and three digits;
//! * the member spans every `IpV4Address`-typed member reachable from a dataflow endpoint
//!   group's endpoints — the `RangedSocketAddress$name` of an agent endpoint's
//!   `ingressAddress` and of the `agentIpAndPortAddress` of an uplink or a downlink agent
//!   endpoint — in BOTH creates of a dataflow endpoint group, plus the two discovery
//!   address lists of `RegisterAgent`, which is answered the SAME sentence for the same
//!   value;
//! * and it stops where the model does: `SocketAddress$name` is a plain modelled `String`
//!   ("Name of a socket address" — `smithy_query(query="shape", shape="SocketAddress")`),
//!   not an `IpV4Address`, so a name carried there is held to no address syntax.
//!
//! Every accepted row is read back through `GetDataflowEndpointGroup`, so an accepted
//! address is asserted from state rather than from the create's answer alone.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::{capability_arn, register};
use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds, get_group};
use crate::dataflow_endpoint_groups_v2::{create_group_v2, downlink, uplink};
use crate::scheduling_loop::{harness, Reply};

/// A port every row's endpoint is addressed at, well inside the ports a socket can be
/// addressed by: no row of this suite is about ports.
const PORT: i32 = 55888;

/// The agent-side port range every row's endpoint carries.
const RANGE: (i32, i32) = (55000, 55100);

/// Addresses the rule ACCEPTS: dotted quads every octet of which is inside 0..255.
///
/// Spans both bounds of `IpV4Address`'s own length (7 and 15 of the modelled 7..16) and
/// values between them, so the table is not one address restated.
const ADDRESSES: [&str; 4] = ["1.2.3.4", "10.0.0.1", "172.31.255.254", "255.255.255.255"];

/// Values the rule REFUSES: each satisfies `IpV4Address`'s pattern — four dot-separated
/// runs of up to three digits — and names no address, the octet past the published 0..255
/// bound falling in each position of the quad and written with one, two and three digits.
const NON_ADDRESSES: [&str; 5] = [
    "256.0.0.1",
    "10.0.0.256",
    "52.0.0.999",
    "0.300.0.1",
    "999.999.999.999",
];

/// Builds one `CreateEndpointDetails` of the V2 create: an endpoint of a given name,
/// addressed at a port, with the agent-side port range.
type EndpointBuilder = fn(&str, i32, (i32, i32)) -> Value;

/// The sentence the service answers for `value`, as both the creates and `RegisterAgent`
/// word it: the value it was given, and the bound each octet of an address keeps.
fn not_an_address_message(value: &str) -> String {
    format!(
        "{value} is not an IPv4 address; each of the four octets of an IPv4 address is \
         between 0 and 255"
    )
}

/// Every `IpV4Address`-typed member reachable from the endpoint the `EndpointDetails`
/// member `member` describes, as a JSON pointer into that member's value.
///
/// Source: `smithy_query(query="closure", operation="CreateDataflowEndpointGroup")` —
/// `RangedSocketAddress$name` is the only `IpV4Address` in the closure, carried by an
/// `AwsGroundStationAgentEndpoint`'s `ingressAddress` and by the `agentIpAndPortAddress` of
/// an uplink's and a downlink's `agentConnectionDetails`. A `DataflowEndpoint`'s own
/// `address` is a `SocketAddress`, whose `name` is a plain `String`, so it carries none.
fn ipv4_members(member: &str) -> Vec<&'static str> {
    match member {
        "endpoint" => Vec::new(),
        "awsGroundStationAgentEndpoint" => vec!["/ingressAddress/socketAddress/name"],
        "uplinkAwsGroundStationAgentEndpoint" | "downlinkAwsGroundStationAgentEndpoint" => {
            vec![
                "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/socketAddress/name",
            ]
        }
        other => panic!("no IpV4Address members are known for the endpoint member {other}"),
    }
}

/// `body` with the string at `pointer` replaced by `value`.
fn with_value(body: &Value, pointer: &str, value: &str) -> Value {
    let mut body = body.clone();
    let slot = body
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("the endpoint body has no member at {pointer}"));
    *slot = json!(value);
    body
}

/// Assert `reply` is the modelled refusal of `value` at `parameter`, worded as every member
/// held to the same shape is answered.
fn assert_not_an_address(label: &str, reply: &Reply, parameter: &str, value: &str) {
    assert_eq!(
        reply.status, 431,
        "case {label}: expected 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "InvalidParameterException",
        "case {label}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!(parameter),
        "case {label}: the refusal must name the invalid parameter, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        not_an_address_message(value),
        "case {label}: the refusal must say which value is not an address and what bound \
         an octet keeps, got {}",
        reply.body
    );
}

/// The one endpoint of the group `id`, as `GetDataflowEndpointGroup` reports it, under the
/// `EndpointDetails` member `member`.
async fn stored_endpoint(router: &MultiServiceRouter, id: &str, member: &str, case: &str) -> Value {
    let read = get_group(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetDataflowEndpointGroup 200 for {id}, got {} {}",
        read.status, read.body
    );
    let endpoints = read
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: endpointsDetails is not a list: {}", read.body))
        .clone();
    assert_eq!(
        endpoints.len(),
        1,
        "case {case}: the group holds the one endpoint it was created with, got {endpoints:?}"
    );
    endpoints[0]
        .get(member)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the stored endpoint carries no {member}: {}",
                endpoints[0]
            )
        })
        .clone()
}

#[tokio::test]
async fn an_endpoint_address_held_to_the_ipv4_shape_is_parsed_as_an_address() {
    let router = harness();

    let mut created: Vec<String> = Vec::new();
    let mut members = 0;
    for kind in endpoint_kinds() {
        for pointer in ipv4_members(kind.member) {
            members += 1;

            for address in ADDRESSES {
                let label = format!("{} at {pointer} = {address}", kind.label);
                let body = with_value(&kind.body, pointer, address);
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {label}: an address every octet of which is inside 0..255 is \
                     accepted, got {} {}",
                    reply.status, reply.body
                );

                // Read back from state: the group reports the address it was created with.
                let id = reply.string("dataflowEndpointGroupId");
                let stored = stored_endpoint(&router, &id, kind.member, &label).await;
                assert_eq!(
                    stored.pointer(pointer),
                    Some(&json!(address)),
                    "case {label}: the stored endpoint must report the address it was \
                     created with, got {stored}"
                );
                created.push(id);
            }

            for value in NON_ADDRESSES {
                let label = format!("{} at {pointer} = {value}", kind.label);
                let body = with_value(&kind.body, pointer, value);
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                let parameter = format!(
                    "endpointDetails[0].{}.{}",
                    kind.member,
                    pointer.trim_start_matches('/').replace('/', ".")
                );
                assert_not_an_address(&label, &reply, &parameter, value);
            }
        }
    }
    assert_eq!(
        members, 3,
        "the three agent endpoint members each carry one IpV4Address, got {members}"
    );

    // Only the accepted creates are stored: every refused one left nothing behind.
    let mut listed = listed_ids(&router, None, created.len()).await;
    listed.sort();
    created.sort();
    assert_eq!(
        listed, created,
        "only the groups that were accepted may be stored: expected {created:?}, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn the_v2_create_holds_its_agent_addresses_to_the_same_shape() {
    let router = harness();

    // Both members of `CreateEndpointDetails`, each carrying the one `IpV4Address` of its
    // `agentIpAndPortAddress`.
    let pointer =
        "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/socketAddress/name";
    let members: [(&str, EndpointBuilder); 2] = [
        ("uplinkAwsGroundStationAgentEndpoint", uplink),
        ("downlinkAwsGroundStationAgentEndpoint", downlink),
    ];

    let mut created: Vec<String> = Vec::new();
    for (member, build) in members {
        let endpoint = build("agent-endpoint", PORT, RANGE);
        let carried = endpoint
            .get(member)
            .unwrap_or_else(|| panic!("the built endpoint names no {member}: {endpoint}"))
            .clone();

        for address in ADDRESSES {
            let label = format!("{member} at {pointer} = {address}");
            let body = with_value(&carried, pointer, address);
            let reply = create_group_v2(&router, json!({"endpoints": [{member: body}]})).await;
            assert_eq!(
                reply.status, 200,
                "case {label}: an address every octet of which is inside 0..255 is \
                 accepted, got {} {}",
                reply.status, reply.body
            );

            let id = reply.string("dataflowEndpointGroupId");
            let stored = stored_endpoint(&router, &id, member, &label).await;
            assert_eq!(
                stored.pointer(pointer),
                Some(&json!(address)),
                "case {label}: the stored endpoint must report the address it was created \
                 with, got {stored}"
            );
            created.push(id);
        }

        for value in NON_ADDRESSES {
            let label = format!("{member} at {pointer} = {value}");
            let body = with_value(&carried, pointer, value);
            let reply = create_group_v2(&router, json!({"endpoints": [{member: body}]})).await;
            let parameter = format!(
                "endpoints[0].{member}.{}",
                pointer.trim_start_matches('/').replace('/', ".")
            );
            assert_not_an_address(&label, &reply, &parameter, value);
        }
    }

    let mut listed = listed_ids(&router, None, created.len()).await;
    listed.sort();
    created.sort();
    assert_eq!(
        listed, created,
        "only the groups that were accepted may be stored: expected {created:?}, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn one_rule_answers_every_member_the_model_types_as_an_ipv4_address() {
    let router = harness();

    // The same value in the one `IpV4Address` of an agent endpoint and in each discovery
    // address list of `RegisterAgent`: one shape, so one sentence.
    let value = NON_ADDRESSES[0];
    let kinds = endpoint_kinds();
    let agent_endpoint = kinds
        .iter()
        .find(|kind| kind.member == "awsGroundStationAgentEndpoint")
        .expect("the endpoint table carries an AWS Ground Station Agent endpoint");
    let body = with_value(
        &agent_endpoint.body,
        "/ingressAddress/socketAddress/name",
        value,
    );
    let refused_create = create_group(
        &router,
        json!({"endpointDetails": [details(agent_endpoint.member, &body, false)]}),
    )
    .await;
    assert_not_an_address(
        "an agent endpoint's ingress address",
        &refused_create,
        "endpointDetails[0].awsGroundStationAgentEndpoint.ingressAddress.socketAddress.name",
        value,
    );

    for member in ["publicIpAddresses", "privateIpAddresses"] {
        let mut discovery = json!({
            "publicIpAddresses": ["52.0.0.1"],
            "privateIpAddresses": ["10.0.0.1"],
            "capabilityArns": [capability_arn(0)],
        });
        discovery[member] = json!([value]);
        let refused_register = register(
            &router,
            json!({
                "agentDetails": {
                    "agentVersion": "1.0.0",
                    "instanceId": "i-0123456789",
                    "instanceType": "m5.large",
                    "agentCpuCores": [0, 1],
                    "componentVersions": [{"componentType": "Agent", "versions": ["1.0.0"]}],
                },
                "discoveryData": discovery,
            }),
        )
        .await;
        assert_not_an_address(
            &format!("a discovery address in {member}"),
            &refused_register,
            &format!("discoveryData.{member}"),
            value,
        );
    }

    // Where the rule STOPS: `SocketAddress$name` is a plain modelled `String`, so a name
    // carried there is not held to the address syntax its ranged sibling is.
    let hostname = "dataflow-endpoint.example.internal";
    let named = with_value(
        &agent_endpoint.body,
        "/egressAddress/socketAddress/name",
        hostname,
    );
    let accepted = create_group(
        &router,
        json!({"endpointDetails": [details(agent_endpoint.member, &named, false)]}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case a socket address named by hostname: `SocketAddress$name` is a plain String, \
         got {} {}",
        accepted.status, accepted.body
    );
    let id = accepted.string("dataflowEndpointGroupId");
    let stored = stored_endpoint(
        &router,
        &id,
        agent_endpoint.member,
        "a socket address named by hostname",
    )
    .await;
    assert_eq!(
        stored.pointer("/egressAddress/socketAddress/name"),
        Some(&json!(hostname)),
        "case a socket address named by hostname: the stored endpoint must report the name \
         it was created with, got {stored}"
    );

    let listed = listed_ids(&router, None, 1).await;
    assert_eq!(
        listed,
        vec![id],
        "only the accepted create may be stored, got {listed:?}"
    );
}
