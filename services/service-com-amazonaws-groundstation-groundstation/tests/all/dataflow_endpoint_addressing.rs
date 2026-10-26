// SPDX-License-Identifier: CC0-1.0

//! How a dataflow endpoint group's endpoints are ADDRESSED: the ports Ground Station
//! connects a stream to, and the names a mission profile selects an endpoint by.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * every port-valued member reachable from an `endpointDetails` item — a dataflow
//!   endpoint's own address, an agent endpoint's egress port and ingress port range, and
//!   the connection pair an uplink or downlink agent endpoint carries — names a port of
//!   the transport (1..=65535) or the create is refused with `InvalidParameterException`
//!   (HTTP 431) and nothing is stored;
//! * a port RANGE additionally has to hold at least one port, so one whose minimum is
//!   above its maximum is refused while a range of a single port is accepted;
//! * each endpoint of a group carries a name, unique WITHIN that group, because that name
//!   is what a mission profile's `dataflowEndpointConfig` selects an endpoint by — the
//!   same name in another group is a different endpoint and is accepted.
//!
//! Every accepted row is read back through `GetDataflowEndpointGroup`, so an accepted
//! address is asserted from state rather than from the create's answer alone.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds, get_group};
use crate::scheduling_loop::harness;

/// The ports a socket address of an endpoint can name: the transport's own port numbers.
///
/// `SocketAddress$port` and the two bounds of `IntegerRange` are plain modelled `Integer`s
/// with no `range` (`smithy_query(query="shape", shape="SocketAddress")`,
/// `smithy_query(query="shape", shape="IntegerRange")`), so these bounds are the service's
/// rather than the model's and the handler is what has to apply them.
const PORT_MIN: i64 = 1;
/// See [`PORT_MIN`].
const PORT_MAX: i64 = 65535;

/// Whether a port-valued member carries ONE port or a port RANGE.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PortShape {
    /// A `SocketAddress$port`.
    Single,
    /// A `RangedSocketAddress$portRange`, an `IntegerRange` of two bounds.
    Range,
}

/// Every port-valued member reachable from the endpoint `member` describes: the JSON
/// pointer into that member's body, and the shape of the value there.
///
/// Source: `smithy_query(query="closure", operation="CreateDataflowEndpointGroup")` —
/// `DataflowEndpoint$address` is a `SocketAddress`; `AwsGroundStationAgentEndpoint` has an
/// `egressAddress` (`ConnectionDetails`) and an `ingressAddress`
/// (`RangedConnectionDetails`); an uplink endpoint's `dataflowDetails` carries
/// `ingressAddressAndPort` + `agentIpAndPortAddress` and a downlink endpoint's carries
/// `egressAddressAndPort` + `agentIpAndPortAddress`.
fn port_members(member: &str) -> Vec<(&'static str, PortShape)> {
    match member {
        "endpoint" => vec![("/address/port", PortShape::Single)],
        "awsGroundStationAgentEndpoint" => vec![
            ("/egressAddress/socketAddress/port", PortShape::Single),
            (
                "/ingressAddress/socketAddress/portRange",
                PortShape::Range,
            ),
        ],
        "uplinkAwsGroundStationAgentEndpoint" => vec![
            (
                "/dataflowDetails/agentConnectionDetails/ingressAddressAndPort/socketAddress/port",
                PortShape::Single,
            ),
            (
                "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/socketAddress/portRange",
                PortShape::Range,
            ),
        ],
        "downlinkAwsGroundStationAgentEndpoint" => vec![
            (
                "/dataflowDetails/agentConnectionDetails/egressAddressAndPort/socketAddress/port",
                PortShape::Single,
            ),
            (
                "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/socketAddress/portRange",
                PortShape::Range,
            ),
        ],
        other => panic!("no port members are known for the endpoint member {other}"),
    }
}

/// `body` with the port-valued member at `pointer` set to `port` — the range shape as the
/// single-port range `[port, port]`, so a row varies the PORT and nothing else.
fn with_port(body: &Value, pointer: &str, shape: PortShape, port: i64) -> Value {
    let mut body = body.clone();
    let slot = body
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("the endpoint body has no member at {pointer}"));
    *slot = match shape {
        PortShape::Single => json!(port),
        PortShape::Range => json!({"minimum": port, "maximum": port}),
    };
    body
}

/// `body` with `name` replaced, or removed entirely when `name` is `None`.
fn with_name(body: &Value, name: Option<&str>) -> Value {
    let mut object: Map<String, Value> = body.as_object().expect("an endpoint object").clone();
    match name {
        Some(name) => {
            object.insert("name".to_string(), json!(name));
        }
        None => {
            object.remove("name");
        }
    }
    Value::Object(object)
}

/// The member path a refusal names for the value at `pointer` of the endpoint member
/// `member`: the request path of the invalid parameter, with the list index of the item.
fn parameter_path(member: &str, pointer: &str) -> String {
    format!(
        "endpointDetails[0].{member}.{}",
        pointer.trim_start_matches('/').replace('/', ".")
    )
}

/// Assert `reply` is this operation's modelled refusal naming `parameter`, and answer its
/// message so a suite can assert what else the refusal has to say.
fn assert_refused(label: &str, reply: &crate::scheduling_loop::Reply, parameter: &str) -> String {
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
    reply.string("message")
}

/// The one endpoint of the group `id`, as `GetDataflowEndpointGroup` reports it.
async fn stored_endpoint(router: &MultiServiceRouter, id: &str, member: &str) -> Value {
    let read = get_group(router, id).await;
    assert_eq!(
        read.status, 200,
        "expected GetDataflowEndpointGroup 200 for {id}, got {} {}",
        read.status, read.body
    );
    let endpoints = read
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| panic!("endpointsDetails is not a list: {}", read.body))
        .clone();
    assert_eq!(
        endpoints.len(),
        1,
        "the group {id} holds the one endpoint it was created with, got {endpoints:?}"
    );
    endpoints[0]
        .get(member)
        .unwrap_or_else(|| panic!("the stored endpoint carries no {member}: {}", endpoints[0]))
        .clone()
}

#[tokio::test]
async fn every_port_an_endpoint_names_must_be_one_the_transport_can_address() {
    let router = harness();

    // The rule spans the port VALUE: each published bound, values inside the range, and
    // values past either bound — including the "any port" placeholder 0, which names no
    // port a stream can be sent to.
    let accepted: [i64; 4] = [PORT_MIN, PORT_MIN + 1, 55888, PORT_MAX];
    let refused: [i64; 5] = [0, -1, -55888, PORT_MAX + 1, 99999];

    let mut created: Vec<String> = Vec::new();
    for kind in endpoint_kinds() {
        for (pointer, shape) in port_members(kind.member) {
            for port in accepted {
                let label = format!("{} at {pointer} = {port}", kind.label);
                let body = with_port(&kind.body, pointer, shape, port);
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {label}: a port inside the transport's range is accepted, got {} {}",
                    reply.status, reply.body
                );

                // Read back from state: the address the group holds is the one the request
                // described, port included.
                let id = reply.string("dataflowEndpointGroupId");
                let stored = stored_endpoint(&router, &id, kind.member).await;
                let expected = match shape {
                    PortShape::Single => json!(port),
                    PortShape::Range => json!({"minimum": port, "maximum": port}),
                };
                assert_eq!(
                    stored.pointer(pointer),
                    Some(&expected),
                    "case {label}: the stored endpoint must report the port it was created \
                     with, got {stored}"
                );
                created.push(id);
            }

            for port in refused {
                let label = format!("{} at {pointer} = {port}", kind.label);
                let body = with_port(&kind.body, pointer, shape, port);
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                // A range's two bounds are judged alike, and the row sets both, so the
                // refusal is about the first of them.
                let parameter = match shape {
                    PortShape::Single => parameter_path(kind.member, pointer),
                    PortShape::Range => {
                        format!("{}.minimum", parameter_path(kind.member, pointer))
                    }
                };
                let message = assert_refused(&label, &reply, &parameter);
                assert!(
                    message.contains(&port.to_string())
                        && message.contains(&PORT_MAX.to_string()),
                    "case {label}: the refusal must say which value and which bounds, got \
                     {message:?}"
                );
            }
        }
    }

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
async fn a_port_range_holds_at_least_one_port() {
    let router = harness();

    // One row per endpoint member that carries a port RANGE, each asked for a range that
    // runs backwards (and so addresses nothing) and then for the narrowest range there is.
    let mut rows = 0;
    for kind in endpoint_kinds() {
        for (pointer, shape) in port_members(kind.member) {
            if shape != PortShape::Range {
                continue;
            }
            rows += 1;

            let mut body = kind.body.clone();
            let slot = body
                .pointer_mut(pointer)
                .unwrap_or_else(|| panic!("the endpoint body has no member at {pointer}"));
            *slot = json!({"minimum": 55100, "maximum": 55000});
            let label = format!("{} with {pointer} running backwards", kind.label);
            let reply = create_group(
                &router,
                json!({"endpointDetails": [details(kind.member, &body, true)]}),
            )
            .await;
            let message = assert_refused(&label, &reply, &parameter_path(kind.member, pointer));
            assert!(
                message.contains("55100") && message.contains("55000"),
                "case {label}: the refusal must name both bounds, got {message:?}"
            );

            // The narrowest usable range is one port wide, which is accepted and stored.
            let body = with_port(&kind.body, pointer, shape, 55000);
            let reply = create_group(
                &router,
                json!({"endpointDetails": [details(kind.member, &body, true)]}),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {} with a one-port range: expected 200, got {} {}",
                kind.label, reply.status, reply.body
            );
            let id = reply.string("dataflowEndpointGroupId");
            let stored = stored_endpoint(&router, &id, kind.member).await;
            assert_eq!(
                stored.pointer(pointer),
                Some(&json!({"minimum": 55000, "maximum": 55000})),
                "case {} with a one-port range: the stored endpoint must report the range it \
                 was created with, got {stored}",
                kind.label
            );
        }
    }
    assert_eq!(
        rows, 3,
        "the three agent endpoint members each carry a port range, got {rows}"
    );

    // One accepted group per row, and nothing from the refusals.
    let listed = listed_ids(&router, None, rows).await;
    assert_eq!(
        listed.len(),
        rows,
        "only the accepted creates may be stored, got {listed:?}"
    );
}

#[tokio::test]
async fn each_endpoint_of_a_group_carries_a_name_no_other_endpoint_of_that_group_has() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];
    let agent = &kinds[1];
    let uplink_agent = &kinds[2];

    // A dataflow endpoint's `name` is the only one the model leaves optional
    // (`smithy_query(query="shape", shape="DataflowEndpoint")` — the three agent endpoint
    // shapes mark theirs `@required`), so that is the member a request can omit.
    let unnamed = create_group(
        &router,
        json!({"endpointDetails": [details(
            dataflow.member,
            &with_name(&dataflow.body, None),
            true,
        )]}),
    )
    .await;
    let message = assert_refused(
        "a dataflow endpoint with no name",
        &unnamed,
        "endpointDetails[0].endpoint.name",
    );
    assert!(
        message.contains("dataflowEndpointConfig"),
        "case a dataflow endpoint with no name: the refusal must say what the name is for, \
         got {message:?}"
    );

    // Two endpoints of ONE group sharing a name: whichever members carry them, and
    // wherever in the list the collision falls.
    let collisions: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "two dataflow endpoints of one name",
            vec![
                details(dataflow.member, &with_name(&dataflow.body, Some("dfe")), true),
                details(dataflow.member, &with_name(&dataflow.body, Some("dfe")), true),
            ],
            "endpointDetails[1].endpoint.name",
        ),
        (
            "two agent endpoints of different members and one name",
            vec![
                details(agent.member, &with_name(&agent.body, Some("shared")), true),
                details(
                    uplink_agent.member,
                    &with_name(&uplink_agent.body, Some("shared")),
                    true,
                ),
            ],
            "endpointDetails[1].uplinkAwsGroundStationAgentEndpoint.name",
        ),
        (
            "the first and third endpoint of one name",
            vec![
                details(agent.member, &with_name(&agent.body, Some("first")), true),
                details(agent.member, &with_name(&agent.body, Some("second")), true),
                details(agent.member, &with_name(&agent.body, Some("first")), true),
            ],
            "endpointDetails[2].awsGroundStationAgentEndpoint.name",
        ),
    ];
    for (label, endpoints, parameter) in &collisions {
        let reply = create_group(&router, json!({"endpointDetails": endpoints})).await;
        let message = assert_refused(label, &reply, parameter);
        assert!(
            message.contains("dataflowEndpointConfig"),
            "case {label}: the refusal must say why the name has to be unique, got \
             {message:?}"
        );
    }

    let listed = listed_ids(&router, None, 0).await;
    assert!(
        listed.is_empty(),
        "a refused create must store nothing, got {listed:?}"
    );

    // What the rule does NOT forbid: names that differ, however slightly — the selection
    // is by the name itself, so two names that are not equal are two endpoints — and the
    // SAME name in another group, the doc's rule being about one group ("each `Config`
    // must match a `DataflowEndpoint` in the same group").
    let accepted: Vec<(&str, Vec<Value>)> = vec![
        (
            "two endpoints whose names differ in case",
            vec![
                details(dataflow.member, &with_name(&dataflow.body, Some("dfe")), true),
                details(dataflow.member, &with_name(&dataflow.body, Some("DFE")), true),
            ],
        ),
        (
            "two endpoints whose names differ in a suffix",
            vec![
                details(
                    dataflow.member,
                    &with_name(&dataflow.body, Some("dfe-1")),
                    true,
                ),
                details(
                    dataflow.member,
                    &with_name(&dataflow.body, Some("dfe-2")),
                    true,
                ),
            ],
        ),
        (
            "a name another group already uses",
            vec![details(
                dataflow.member,
                &with_name(&dataflow.body, Some("dfe")),
                true,
            )],
        ),
        (
            "the same name again in a third group",
            vec![details(
                dataflow.member,
                &with_name(&dataflow.body, Some("dfe")),
                true,
            )],
        ),
    ];
    let mut created = Vec::new();
    for (label, endpoints) in &accepted {
        let reply = create_group(&router, json!({"endpointDetails": endpoints})).await;
        assert_eq!(
            reply.status, 200,
            "case {label}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let id = reply.string("dataflowEndpointGroupId");

        // Read back from state: every endpoint of the group reports the name it was
        // created with, so the names a `dataflowEndpointConfig` would select by are the
        // request's.
        let read = get_group(&router, &id).await;
        let names: Vec<String> = read
            .field("endpointsDetails")
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: endpointsDetails is not a list: {}", read.body))
            .iter()
            .map(|item| {
                item.get(dataflow.member)
                    .or_else(|| item.get(agent.member))
                    .and_then(|endpoint| endpoint.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {label}: a stored endpoint with no name: {item}"))
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = endpoints
            .iter()
            .map(|item| {
                item.get(dataflow.member)
                    .or_else(|| item.get(agent.member))
                    .and_then(|endpoint| endpoint.get("name"))
                    .and_then(Value::as_str)
                    .expect("a request endpoint carries a name")
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, expected,
            "case {label}: the group must report the endpoint names it was created with, \
             got {names:?}"
        );
        created.push(id);
    }

    created.sort();
    let mut listed = listed_ids(&router, None, created.len()).await;
    listed.sort();
    assert_eq!(
        listed, created,
        "every accepted group must be stored: expected {created:?}, got {listed:?}"
    );
}
