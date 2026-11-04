// SPDX-License-Identifier: CC0-1.0

//! `CreateDataflowEndpointGroupV2`, end to end over HTTP through the same front door as
//! the rest of this target's suites (the harness, signed `restJson1` requests and the
//! response reader come from [`crate::scheduling_loop`]; the group reader, the listing and
//! the delete from the sibling group suites).
//!
//! The RULE under test, one level above any single call: this create mints a dataflow
//! endpoint group of the account from the Ground Station Agent endpoints the request
//! describes and persists it, so the group is afterwards addressed by every operation of
//! the `dataflow-endpoint-group/{dataflowEndpointGroupId}` resource — its read, its listing
//! and its delete — reporting the endpoints the request carried plus the members the
//! SERVICE owns (`agentStatus`, `auditResults`, `healthStatus`, `healthReasons`), which an
//! agent endpoint takes from the agent registered for the group.
//!
//! Asserted over tables of inputs rather than one call:
//!
//! * every member of `CreateEndpointDetails`, alone, together in one group, and at the
//!   modelled maximum of twelve endpoints, with the pre/post-pass durations absent and at
//!   each published bound, with and without tags;
//! * a created group is listed among the account's groups beside one the other create made,
//!   and a deleted one is gone from every read;
//! * an endpoint's health follows the agent registered for the group's ARN, since the
//!   group's ARN is the capability an agent registers for;
//! * an endpoint naming a port no socket can be addressed by, a port range that runs
//!   backwards, or a name another endpoint of the same group already took is refused
//!   `InvalidParameterException` (HTTP 431) with nothing persisted;
//! * the model's own rules — the endpoint-count bounds, the duration range and the endpoint
//!   name's pattern — are refused by the request validator with nothing persisted.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    AGENT_ACTIVE, AGENT_INACTIVE, AUDIT_HEALTHY, BAD_REQUEST_ERROR_CODE, HEALTH_HEALTHY,
    HEALTH_UNHEALTHY, INVALID_REQUEST_BODY, PORT_MAX, PORT_MIN, REASON_HEALTHY,
    REASON_NO_REGISTERED_AGENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::{register, update_status, uuid_from};
use crate::dataflow_endpoint_group_deletion::assert_group_absent;
use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{
    create_group, delete_group, details, endpoint_kinds, expected_arn, get_group, DURATION_MAX,
    DURATION_MIN,
};
use crate::scheduling_loop::{harness, is_uuid, send, Reply};

/// The bounds of `CreateEndpointDetailsList` (len 1..12 — `smithy_query(query="shape",
/// shape="CreateEndpointDetailsList")`): the fewest and the most endpoints one group of
/// this create may hold.
const ENDPOINTS_MIN: usize = 1;
/// See [`ENDPOINTS_MIN`].
const ENDPOINTS_MAX: usize = 12;

/// A port every row's endpoint is addressed at unless the row is about ports, well inside
/// [`PORT_MIN`]..=[`PORT_MAX`].
const PORT: i32 = 55888;

/// The agent-side port range every row's endpoint carries unless the row is about ports.
const RANGE: (i32, i32) = (55000, 55100);

/// `POST /dataflowEndpointGroupV2`.
async fn create_group_v2(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/dataflowEndpointGroupV2", Some(body)).await
}

/// One `CreateEndpointDetails` naming an uplink AWS Ground Station Agent endpoint: the
/// address Ground Station sends the uplink stream to, and the agent's own address and port
/// range (`smithy_query(query="shape", shape="UplinkConnectionDetails")`).
fn uplink(name: &str, port: i32, range: (i32, i32)) -> Value {
    json!({"uplinkAwsGroundStationAgentEndpoint": {
        "name": name,
        "dataflowDetails": {"agentConnectionDetails": {
            "ingressAddressAndPort": {
                "socketAddress": {"name": "172.31.0.30", "port": port},
                "mtu": 1500,
            },
            "agentIpAndPortAddress": {
                "socketAddress": {
                    "name": "172.31.0.31",
                    "portRange": {"minimum": range.0, "maximum": range.1},
                },
                "mtu": 1500,
            },
        }},
    }})
}

/// One `CreateEndpointDetails` naming a downlink AWS Ground Station Agent endpoint: the
/// agent's own address and port range, and the address the downlink stream leaves from
/// (`smithy_query(query="shape", shape="DownlinkConnectionDetails")`).
fn downlink(name: &str, port: i32, range: (i32, i32)) -> Value {
    json!({"downlinkAwsGroundStationAgentEndpoint": {
        "name": name,
        "dataflowDetails": {"agentConnectionDetails": {
            "agentIpAndPortAddress": {
                "socketAddress": {
                    "name": "172.31.0.41",
                    "portRange": {"minimum": range.0, "maximum": range.1},
                },
                "mtu": 1400,
            },
            "egressAddressAndPort": {
                "socketAddress": {"name": "172.31.0.40", "port": port},
                "mtu": 1400,
            },
        }},
    }})
}

/// One member of the `CreateEndpointDetails` union, and how a request carries an endpoint
/// in it.
struct Member {
    /// The modelled member name, as the wire spells it.
    name: &'static str,
    /// Builds the union value for an endpoint called `name`, addressed at `port`, with the
    /// agent port range `range`.
    build: fn(&str, i32, (i32, i32)) -> Value,
}

/// Both members of `CreateEndpointDetails` (source: `smithy_query(query="shape",
/// shape="CreateEndpointDetails")` — `uplinkAwsGroundStationAgentEndpoint` and
/// `downlinkAwsGroundStationAgentEndpoint`, and no others: this create takes Ground Station
/// Agent based endpoints only).
fn members() -> Vec<Member> {
    vec![
        Member {
            name: "uplinkAwsGroundStationAgentEndpoint",
            build: uplink,
        },
        Member {
            name: "downlinkAwsGroundStationAgentEndpoint",
            build: downlink,
        },
    ]
}

/// The `index`th endpoint of a group spanning both members, named distinctly so one group
/// may hold as many as the model allows.
fn endpoint_at(index: usize) -> Value {
    let members = members();
    let member = &members[index % members.len()];
    (member.build)(
        &format!("agent-endpoint-{index}"),
        PORT + index as i32,
        (RANGE.0 + index as i32, RANGE.1 + index as i32),
    )
}

/// One row of the group table: a group described a different way, so the rule is asserted
/// over a class of create requests rather than one shape of body.
struct Row {
    /// Names the row in every assertion.
    label: String,
    /// The `endpoints` list the create carries.
    endpoints: Vec<Value>,
    /// `contactPrePassDurationSeconds` / `contactPostPassDurationSeconds`, when set.
    durations: Option<(i64, i64)>,
    /// The `tags` the create carries, when any.
    tags: Option<Value>,
}

/// Groups differing along everything this create may vary: which member each endpoint is
/// carried in, how many endpoints the group holds (both bounds of
/// `CreateEndpointDetailsList` and a value between them), and the pre/post-pass durations
/// absent and at each published bound of `DataflowEndpointGroupDurationInSeconds` (range
/// 30..480 — `smithy_query(query="shape", shape="DataflowEndpointGroupDurationInSeconds")`,
/// read through the sibling suite's constants).
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            label: "one uplink endpoint, no durations, no tags".to_string(),
            endpoints: vec![uplink("uplink-only", PORT, RANGE)],
            durations: None,
            tags: None,
        },
        Row {
            label: "one downlink endpoint at both duration bounds, with tags".to_string(),
            endpoints: vec![downlink("downlink-only", PORT, RANGE)],
            durations: Some((DURATION_MIN, DURATION_MAX)),
            tags: Some(json!({"mission": "downlink", "stage": "created"})),
        },
        Row {
            label: "an uplink and a downlink endpoint in one group".to_string(),
            endpoints: vec![
                uplink("pair-uplink", PORT, RANGE),
                downlink("pair-downlink", PORT + 1, RANGE),
            ],
            durations: Some((DURATION_MAX, DURATION_MIN)),
            tags: None,
        },
    ];
    for count in [ENDPOINTS_MIN, 7, ENDPOINTS_MAX] {
        rows.push(Row {
            label: format!("{count} endpoints spanning both members"),
            endpoints: (0..count).map(endpoint_at).collect(),
            durations: Some((DURATION_MIN, DURATION_MIN)),
            tags: Some(json!({"endpoints": count.to_string()})),
        });
    }
    rows
}

/// The `CreateDataflowEndpointGroupV2` body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("endpoints".to_string(), json!(row.endpoints.clone()));
    if let Some((pre, post)) = row.durations {
        body.insert("contactPrePassDurationSeconds".to_string(), json!(pre));
        body.insert("contactPostPassDurationSeconds".to_string(), json!(post));
    }
    if let Some(tags) = &row.tags {
        body.insert("tags".to_string(), tags.clone());
    }
    Value::Object(body)
}

/// The one member of `endpoints[index]`'s union, and the endpoint object carried in it.
fn requested_endpoint(endpoints: &[Value], index: usize) -> (String, Value) {
    let object = endpoints[index]
        .as_object()
        .unwrap_or_else(|| panic!("endpoints[{index}] is not a union object"));
    let (member, value) = object
        .iter()
        .next()
        .unwrap_or_else(|| panic!("endpoints[{index}] names no union member"));
    (member.clone(), value.clone())
}

/// Create the group `row` describes and answer the id the service minted for it.
async fn create_row(router: &MultiServiceRouter, row: &Row) -> String {
    let reply = create_group_v2(router, create_body(row)).await;
    assert_eq!(
        reply.status, 200,
        "case {}: expected CreateDataflowEndpointGroupV2 200, got {} {}",
        row.label, reply.status, reply.body
    );
    let id = reply.string("dataflowEndpointGroupId");
    assert!(
        is_uuid(&id),
        "case {}: the minted id must be a lowercase uuid, got {id:?}",
        row.label
    );
    id
}

/// The endpoints a read of `id` reports.
async fn read_endpoints(router: &MultiServiceRouter, id: &str, case: &str) -> Vec<Value> {
    let reply = get_group(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetDataflowEndpointGroup 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: endpointsDetails is not a list: {}", reply.body))
        .clone()
}

#[tokio::test]
async fn a_created_group_reads_back_every_endpoint_it_was_created_with() {
    let router = harness();

    for row in rows() {
        let id = create_row(&router, &row).await;

        let reply = get_group(&router, &id).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected GetDataflowEndpointGroup 200, got {} {}",
            row.label, reply.status, reply.body
        );
        assert_eq!(
            reply.string("dataflowEndpointGroupId"),
            id,
            "case {}: the group must read back under the id it was created with, got {}",
            row.label,
            reply.body
        );
        assert_eq!(
            reply.string("dataflowEndpointGroupArn"),
            expected_arn(&id),
            "case {}: wrong ARN, got {}",
            row.label,
            reply.body
        );
        let (expected_pre, expected_post) = match row.durations {
            Some((pre, post)) => (json!(pre), json!(post)),
            None => (Value::Null, Value::Null),
        };
        assert_eq!(
            reply.field("contactPrePassDurationSeconds"),
            &expected_pre,
            "case {}: wrong contactPrePassDurationSeconds, got {}",
            row.label,
            reply.body
        );
        assert_eq!(
            reply.field("contactPostPassDurationSeconds"),
            &expected_post,
            "case {}: wrong contactPostPassDurationSeconds, got {}",
            row.label,
            reply.body
        );
        // A read of this service always serializes the tag map, `{}` for a resource
        // carrying no tag (the `state::response_tags` reader every read goes through).
        assert_eq!(
            reply.field("tags"),
            row.tags.as_ref().unwrap_or(&json!({})),
            "case {}: wrong tags, got {}",
            row.label,
            reply.body
        );

        let endpoints = read_endpoints(&router, &id, &row.label).await;
        assert_eq!(
            endpoints.len(),
            row.endpoints.len(),
            "case {}: the group holds the endpoints it was created with, got {endpoints:?}",
            row.label
        );
        for (index, endpoint) in endpoints.iter().enumerate() {
            let (member, requested) = requested_endpoint(&row.endpoints, index);
            // What the request described, plus the one member the SERVICE owns on an agent
            // endpoint that no agent has registered for: its `agentStatus`. No
            // `auditResults` is reported, no audit having run.
            let mut expected = requested
                .as_object()
                .expect("an endpoint object")
                .clone();
            expected.insert("agentStatus".to_string(), json!(AGENT_INACTIVE));
            assert_eq!(
                endpoint.get(&member),
                Some(&Value::Object(expected)),
                "case {}: endpoint {index} must read back as it was created plus the \
                 service's own agentStatus, got {endpoint}",
                row.label
            );
            // The VPC placement triple belongs to a Ground-Station-connected dataflow
            // endpoint; this create describes none, so the stored endpoint carries none.
            assert_eq!(
                endpoint.get("securityDetails"),
                None,
                "case {}: endpoint {index} was created with no securityDetails, got {endpoint}",
                row.label
            );
            assert_eq!(
                endpoint.get("healthStatus").and_then(Value::as_str),
                Some(HEALTH_UNHEALTHY),
                "case {}: endpoint {index} has no registered agent yet, got {endpoint}",
                row.label
            );
            assert_eq!(
                endpoint.get("healthReasons"),
                Some(&json!([REASON_NO_REGISTERED_AGENT])),
                "case {}: wrong healthReasons on endpoint {index}, got {endpoint}",
                row.label
            );
        }
    }
}

#[tokio::test]
async fn a_group_this_create_minted_is_one_of_the_accounts_groups() {
    let router = harness();
    let rows = rows();

    // One group per row, plus one made by the OTHER create of this resource: both creates
    // write the one dataflow endpoint group store, so the listing reports them together.
    let mut created: Vec<(String, String)> = Vec::new();
    for row in &rows {
        created.push((row.label.clone(), create_row(&router, row).await));
    }
    let dataflow = &endpoint_kinds()[0];
    let other = create_group(
        &router,
        json!({"endpointDetails": [details(dataflow.member, &dataflow.body, true)]}),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "case a group of the other create: expected 200, got {} {}",
        other.status, other.body
    );
    created.push((
        "a group of the other create".to_string(),
        other.string("dataflowEndpointGroupId"),
    ));

    let listed = listed_ids(&router, None, created.len()).await;
    for (label, id) in &created {
        assert_eq!(
            listed.iter().filter(|listed| *listed == id).count(),
            1,
            "case {label}: the group must be listed exactly once, got {listed:?}"
        );
    }

    // A group this create minted is deleted through the delete of the same resource, and is
    // then gone from every read, while the groups beside it are untouched.
    let (label, deleted) = created.remove(0);
    let reply = delete_group(&router, &deleted).await;
    assert_eq!(
        reply.status, 200,
        "case {label}: expected DeleteDataflowEndpointGroup 200, got {} {}",
        reply.status, reply.body
    );
    assert_group_absent(&label, &get_group(&router, &deleted).await);
    let remaining = listed_ids(&router, Some(2), created.len()).await;
    assert!(
        !remaining.contains(&deleted),
        "case {label}: a deleted group must be gone from the listing, got {remaining:?}"
    );
    for (label, id) in &created {
        assert!(
            remaining.contains(id),
            "case {label}: deleting another group must leave this one listed, got {remaining:?}"
        );
    }
}

#[tokio::test]
async fn the_endpoints_of_a_created_group_follow_the_agent_registered_for_its_arn() {
    let router = harness();

    for member in members() {
        let endpoint = (member.build)("carried-endpoint", PORT, RANGE);
        let reply = create_group_v2(&router, json!({"endpoints": [endpoint]})).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200, got {} {}",
            member.name, reply.status, reply.body
        );
        let id = reply.string("dataflowEndpointGroupId");

        // An agent registers for the capabilities it manages, and the capability an
        // agent-carried endpoint belongs to is the dataflow endpoint group that holds it,
        // addressed by the ARN this create minted for it.
        let registered = register(
            &router,
            json!({
                "agentDetails": {
                    "agentVersion": "1.4.2",
                    "instanceId": "i-00000000000000aa",
                    "instanceType": "m5.large",
                    "agentCpuCores": [0, 1],
                    "componentVersions": [{"componentType": "Agent", "versions": ["1.4.2"]}],
                },
                "discoveryData": {
                    "publicIpAddresses": ["52.0.0.1"],
                    "privateIpAddresses": ["10.0.0.1"],
                    "capabilityArns": [expected_arn(&id)],
                },
            }),
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "case {}: expected RegisterAgent 200, got {} {}",
            member.name, registered.status, registered.body
        );
        let agent_id = registered.string("agentId");

        let reported = update_status(
            &router,
            &agent_id,
            json!({
                "taskId": uuid_from(0x7a5c_0002, 1),
                "aggregateStatus": {"status": AGENT_ACTIVE},
                "componentStatuses": [],
            }),
        )
        .await;
        assert_eq!(
            reported.status, 200,
            "case {}: expected UpdateAgentStatus 200, got {} {}",
            member.name, reported.status, reported.body
        );

        let endpoints = read_endpoints(&router, &id, member.name).await;
        let carried = &endpoints[0];
        let reported = carried
            .get(member.name)
            .unwrap_or_else(|| panic!("case {}: the read lost the endpoint: {carried}", member.name));
        assert_eq!(
            reported.get("agentStatus").and_then(Value::as_str),
            Some(AGENT_ACTIVE),
            "case {}: the endpoint must report the agent's own status, got {carried}",
            member.name
        );
        assert_eq!(
            reported.get("auditResults").and_then(Value::as_str),
            Some(AUDIT_HEALTHY),
            "case {}: an endpoint whose agent is running passes its audit, got {carried}",
            member.name
        );
        assert_eq!(
            carried.get("healthStatus").and_then(Value::as_str),
            Some(HEALTH_HEALTHY),
            "case {}: the endpoint's stream is flowing, got {carried}",
            member.name
        );
        assert_eq!(
            carried.get("healthReasons"),
            Some(&json!([REASON_HEALTHY])),
            "case {}: wrong healthReasons, got {carried}",
            member.name
        );
    }
}

#[tokio::test]
async fn an_endpoint_no_stream_could_reach_is_refused_and_nothing_is_stored() {
    let router = harness();

    // A group the account already holds, so every refusal below is asserted against a
    // store it must leave exactly as it found it.
    let held = create_row(&router, &rows()[0]).await;

    let mut refused: Vec<(String, Value)> = vec![
        (
            "two endpoints sharing a name".to_string(),
            json!([
                uplink("same-name", PORT, RANGE),
                downlink("same-name", PORT + 1, RANGE),
            ]),
        ),
        (
            "an agent port range running backwards".to_string(),
            json!([uplink("backwards-range", PORT, (RANGE.1, RANGE.0))]),
        ),
    ];
    // Each member's endpoint at a port no socket can be addressed by: one below the first
    // port a transport names, one above the last, and the same two as the bounds of the
    // agent's port range.
    for member in members() {
        for (label, port, range) in [
            ("a port below the first", PORT_MIN - 1, RANGE),
            ("a port above the last", PORT_MAX + 1, RANGE),
            (
                "a range minimum below the first port",
                PORT,
                (PORT_MIN - 1, RANGE.1),
            ),
            (
                "a range maximum above the last port",
                PORT,
                (RANGE.0, PORT_MAX + 1),
            ),
        ] {
            refused.push((
                format!("{label} on {}", member.name),
                json!([(member.build)("out-of-range", port, range)]),
            ));
        }
    }

    for (label, endpoints) in &refused {
        let reply = create_group_v2(&router, json!({"endpoints": endpoints})).await;
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
        let parameter = reply.string("parameterName");
        assert!(
            parameter.starts_with("endpoints["),
            "case {label}: the refusal must name the endpoint at fault, got {}",
            reply.body
        );
    }

    // A tag map the platform refuses is refused before an id is minted too, naming the
    // member it is about.
    let reply = create_group_v2(
        &router,
        json!({
            "endpoints": [uplink("reserved-tag", PORT, RANGE)],
            "tags": {"aws:owner": "someone"},
        }),
    )
    .await;
    assert_eq!(
        reply.status, 431,
        "case a reserved tag key: expected 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!("tags"),
        "case a reserved tag key: the refusal must name the member, got {}",
        reply.body
    );

    // Every refusal left the store exactly as it was: the one group the account held.
    let listed = listed_ids(&router, None, 1).await;
    assert_eq!(
        listed,
        vec![held],
        "a refused create must persist nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn the_models_own_rules_are_refused_by_the_request_validator() {
    let router = harness();
    let held = create_row(&router, &rows()[0]).await;

    // Each row breaks ONE modelled rule of this operation's input: the list bounds, the
    // duration range, the endpoint name's pattern and length, and the required members.
    let bodies: Vec<(String, Value)> = vec![
        (
            "no endpoints at all".to_string(),
            json!({"endpoints": []}),
        ),
        (
            format!("one endpoint past the maximum of {ENDPOINTS_MAX}"),
            json!({"endpoints": (0..=ENDPOINTS_MAX).map(endpoint_at).collect::<Vec<_>>()}),
        ),
        (
            format!("a pre-pass duration one below {DURATION_MIN}"),
            json!({
                "endpoints": [uplink("below-duration", PORT, RANGE)],
                "contactPrePassDurationSeconds": DURATION_MIN - 1,
            }),
        ),
        (
            format!("a post-pass duration one above {DURATION_MAX}"),
            json!({
                "endpoints": [uplink("above-duration", PORT, RANGE)],
                "contactPostPassDurationSeconds": DURATION_MAX + 1,
            }),
        ),
        (
            "an endpoint name outside the modelled pattern".to_string(),
            json!({"endpoints": [uplink("not/a/safe/name", PORT, RANGE)]}),
        ),
        (
            "an endpoint with no name".to_string(),
            json!({"endpoints": [{"uplinkAwsGroundStationAgentEndpoint": {
                "dataflowDetails": {"agentConnectionDetails": {
                    "ingressAddressAndPort": {
                        "socketAddress": {"name": "172.31.0.30", "port": PORT},
                    },
                    "agentIpAndPortAddress": {
                        "socketAddress": {
                            "name": "172.31.0.31",
                            "portRange": {"minimum": RANGE.0, "maximum": RANGE.1},
                        },
                    },
                }},
            }}]}),
        ),
        (
            "an endpoint with no dataflowDetails".to_string(),
            json!({"endpoints": [
                {"uplinkAwsGroundStationAgentEndpoint": {"name": "no-dataflow-details"}},
            ]}),
        ),
    ];

    for (label, body) in &bodies {
        let reply = create_group_v2(&router, body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            BAD_REQUEST_ERROR_CODE,
            "case {label}: expected {BAD_REQUEST_ERROR_CODE}, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            INVALID_REQUEST_BODY,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
    }

    // The endpoint count at each end of the published bounds is accepted, so the refusals
    // above are the bounds and not the rule.
    for count in [ENDPOINTS_MIN, ENDPOINTS_MAX] {
        let reply = create_group_v2(
            &router,
            json!({"endpoints": (0..count).map(endpoint_at).collect::<Vec<_>>()}),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {count} endpoints: expected 200, got {} {}",
            reply.status, reply.body
        );
    }

    let listed = listed_ids(&router, None, 3).await;
    assert!(
        listed.contains(&held),
        "the group the account held must still be listed, got {listed:?}"
    );
}
