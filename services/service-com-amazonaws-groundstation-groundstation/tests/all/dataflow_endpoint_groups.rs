// SPDX-License-Identifier: CC0-1.0

//! Dataflow endpoint groups, end to end over HTTP through the same front door as the
//! rest of this target's suites (the harness, signed `restJson1` requests and the
//! response reader come from [`crate::scheduling_loop`]).
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * a created group is read back from state: `GetDataflowEndpointGroup` answers the id
//!   and ARN that were minted, the endpoint the request described, its `securityDetails`,
//!   its tags and its pre/post-pass durations — asserted for every one of the four
//!   endpoint members `EndpointDetails` declares;
//! * `healthStatus`/`healthReasons` are "ignored when calling
//!   `CreateDataflowEndpointGroup`" (the model's own words), so the service reports the
//!   health IT observes whatever the request asked for;
//! * `securityDetails` is the VPC placement triple Ground Station needs to connect a
//!   DATAFLOW endpoint's stream, so only that kind is refused without it, while an AWS
//!   Ground Station Agent endpoint — whose stream the customer's own agent carries — is
//!   created from its own required members alone;
//! * each `EndpointDetails` names exactly one endpoint, and one group may not mix
//!   AWS Ground Station Agent endpoints with dataflow endpoints — each refused with
//!   `InvalidParameterException` (HTTP 431), and nothing stored;
//! * a dataflow endpoint is admitted `creating` and becomes `created` when the clock
//!   makes that move due, while an agent endpoint no agent has registered for stays
//!   `NO_REGISTERED_AGENT`;
//! * the pre/post-pass durations round-trip at each published bound and a value past
//!   either one is refused by the model's own rules;
//! * `ListDataflowEndpointGroups` returns every group exactly once across its pages, and
//!   a deleted group is gone from every read.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    AGENT_INACTIVE, BAD_REQUEST_ERROR_CODE, ENDPOINT_CREATED, HEALTH_HEALTHY, HEALTH_UNHEALTHY,
    INVALID_REQUEST_BODY, REASON_DATAPLANE_FAILURE, REASON_HEALTHY, REASON_NO_REGISTERED_AGENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_deletion::assert_group_absent;
use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::scheduling_loop::{harness, is_uuid, send, Reply, ABSENT_UUID, AFTER_SETTLE, REGION};

/// The published bounds of `DataflowEndpointGroupDurationInSeconds` (range 30..480 —
/// `smithy_query(query="shape", shape="DataflowEndpointGroupDurationInSeconds")`).
///
/// `pub(crate)` so the sibling suites assert the same published bounds from one place.
pub(crate) const DURATION_MIN: i64 = 30;
/// See [`DURATION_MIN`].
pub(crate) const DURATION_MAX: i64 = 480;

/// One row of the endpoint-kind table: a member of `EndpointDetails` that describes an
/// endpoint, the value a request carries in it, and what the SERVICE adds to it.
///
/// `pub(crate)` here and on [`endpoint_kinds`] so the deletion suite spans the same four
/// endpoint kinds from this one table rather than keeping a second copy of it.
pub(crate) struct Kind {
    /// Names the row in every assertion.
    pub(crate) label: &'static str,
    /// The modelled member name, as the wire spells it.
    pub(crate) member: &'static str,
    /// The member's value as the request carries it.
    pub(crate) body: Value,
    /// The member the service fills in on the endpoint itself, and the value it reports.
    derived: (&'static str, Value),
    /// The `CapabilityHealth` and the one `CapabilityHealthReason` the service reports for
    /// a freshly created endpoint of this kind.
    health_status: &'static str,
    health_reason: &'static str,
    /// Whether an endpoint of this kind can only be created WITH `securityDetails`.
    ///
    /// `securityDetails` carries "a list of subnets, a list of security groups and a role
    /// to connect streams to instances" (`smithy_query(query="doc",
    /// shape="SecurityDetails")`) — the elastic network interfaces Ground Station places
    /// to deliver a `DataflowEndpoint`'s stream. An agent endpoint's stream is carried by
    /// the agent the customer runs on their own instance, and `EndpointDetails` leaves the
    /// member optional (`smithy_query(query="shape", shape="EndpointDetails")`), so one is
    /// created without it.
    needs_security_details: bool,
}

/// The four endpoint members of `EndpointDetails`, one row each.
///
/// Source: `smithy_query(query="shape", shape="EndpointDetails")` — `endpoint`,
/// `awsGroundStationAgentEndpoint`, `uplinkAwsGroundStationAgentEndpoint` and
/// `downlinkAwsGroundStationAgentEndpoint` — and the shape of each member's own target
/// (`smithy_query(query="closure", operation="CreateDataflowEndpointGroup")`).
pub(crate) fn endpoint_kinds() -> Vec<Kind> {
    vec![
        Kind {
            label: "a dataflow endpoint",
            member: "endpoint",
            body: json!({
                "name": "downlink-demod-decode",
                "address": {"name": "172.31.0.10", "port": 55888},
                "mtu": 1500,
            }),
            // A dataflow endpoint is handed back attached: the recorded group lifecycle
            // reads a created group and is answered `created` / HEALTHY / [HEALTHY].
            derived: ("status", json!(ENDPOINT_CREATED)),
            health_status: HEALTH_HEALTHY,
            health_reason: REASON_HEALTHY,
            needs_security_details: true,
        },
        Kind {
            label: "an AWS Ground Station Agent endpoint",
            member: "awsGroundStationAgentEndpoint",
            body: json!({
                "name": "agent-endpoint",
                "egressAddress": {
                    "socketAddress": {"name": "172.31.0.20", "port": 55888},
                    "mtu": 1500,
                },
                "ingressAddress": {
                    "socketAddress": {
                        "name": "172.31.0.21",
                        "portRange": {"minimum": 55000, "maximum": 55100},
                    },
                    "mtu": 1400,
                },
            }),
            derived: ("agentStatus", json!(AGENT_INACTIVE)),
            health_status: HEALTH_UNHEALTHY,
            health_reason: REASON_NO_REGISTERED_AGENT,
            needs_security_details: false,
        },
        Kind {
            label: "an uplink agent endpoint",
            member: "uplinkAwsGroundStationAgentEndpoint",
            body: json!({
                "name": "uplink-agent-endpoint",
                "dataflowDetails": {"agentConnectionDetails": {
                    "ingressAddressAndPort": {
                        "socketAddress": {"name": "172.31.0.30", "port": 55888},
                        "mtu": 1500,
                    },
                    "agentIpAndPortAddress": {
                        "socketAddress": {
                            "name": "172.31.0.31",
                            "portRange": {"minimum": 55000, "maximum": 55100},
                        },
                        "mtu": 1500,
                    },
                }},
            }),
            derived: ("agentStatus", json!(AGENT_INACTIVE)),
            health_status: HEALTH_UNHEALTHY,
            health_reason: REASON_NO_REGISTERED_AGENT,
            needs_security_details: false,
        },
        Kind {
            label: "a downlink agent endpoint",
            member: "downlinkAwsGroundStationAgentEndpoint",
            body: json!({
                "name": "downlink-agent-endpoint",
                "dataflowDetails": {"agentConnectionDetails": {
                    "agentIpAndPortAddress": {
                        "socketAddress": {
                            "name": "172.31.0.41",
                            "portRange": {"minimum": 55000, "maximum": 55100},
                        },
                        "mtu": 1400,
                    },
                    "egressAddressAndPort": {
                        "socketAddress": {"name": "172.31.0.40", "port": 55888},
                        "mtu": 1400,
                    },
                }},
            }),
            derived: ("agentStatus", json!(AGENT_INACTIVE)),
            health_status: HEALTH_UNHEALTHY,
            health_reason: REASON_NO_REGISTERED_AGENT,
            needs_security_details: false,
        },
    ]
}

/// The subnets, security groups and role Ground Station connects an endpoint's streams
/// with (`SecurityDetails`, whose three members are all required; the role ARN satisfies
/// the modelled `RoleArn` pattern).
pub(crate) fn security_details() -> Value {
    json!({
        "subnetIds": ["subnet-0123456789abcdef0"],
        "securityGroupIds": ["sg-0123456789abcdef0"],
        "roleArn": format!(
            "arn:aws:iam::{}:role/GroundStationDataflowRole",
            engine::emulator_account_id()
        ),
    })
}

/// One `EndpointDetails`: the endpoint `member` carries, with the group's security
/// details when `with_security`.
pub(crate) fn details(member: &str, body: &Value, with_security: bool) -> Value {
    let mut item = Map::new();
    if with_security {
        item.insert("securityDetails".to_string(), security_details());
    }
    item.insert(member.to_string(), body.clone());
    Value::Object(item)
}

/// `body` with `key` set to `value` — the endpoint as the service reports it back, which
/// is what the request carried plus the member the service owns.
fn with_member(body: &Value, key: &str, value: &Value) -> Value {
    let mut echoed = body.as_object().expect("an endpoint object").clone();
    echoed.insert(key.to_string(), value.clone());
    Value::Object(echoed)
}

/// `POST /dataflowEndpointGroup`.
pub(crate) async fn create_group(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/dataflowEndpointGroup", Some(body)).await
}

/// `GET /dataflowEndpointGroup/{dataflowEndpointGroupId}`.
pub(crate) async fn get_group(router: &MultiServiceRouter, id: &str) -> Reply {
    send(router, "GET", &format!("/dataflowEndpointGroup/{id}"), None).await
}

/// `DELETE /dataflowEndpointGroup/{dataflowEndpointGroupId}`.
pub(crate) async fn delete_group(router: &MultiServiceRouter, id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/dataflowEndpointGroup/{id}"),
        None,
    )
    .await
}

/// `GET /dataflowEndpointGroup`, with `query` appended verbatim (`""` for no filters).
pub(crate) async fn list_groups(router: &MultiServiceRouter, query: &str) -> Reply {
    send(router, "GET", &format!("/dataflowEndpointGroup{query}"), None).await
}

/// The ARN this service mints for a group: `dataflow-endpoint-group/<uuid>` under the
/// request's region and the emulator's account.
pub(crate) fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:dataflow-endpoint-group/{id}",
        engine::emulator_account_id()
    )
}

#[tokio::test]
async fn a_created_group_reads_back_every_endpoint_kind_it_was_created_with() {
    let router = harness();

    for kind in endpoint_kinds() {
        let Kind {
            label,
            member,
            body,
            derived,
            health_status,
            health_reason,
            needs_security_details: _,
        } = kind;
        let request = json!({
            "endpointDetails": [details(member, &body, true)],
            "contactPrePassDurationSeconds": 120,
            "contactPostPassDurationSeconds": 180,
            "tags": {"stage": "created"},
        });

        let created = create_group(&router, request).await;
        assert_eq!(
            created.status, 200,
            "case {label}: expected CreateDataflowEndpointGroup 200, got {} {}",
            created.status, created.body
        );
        let id = created.string("dataflowEndpointGroupId");
        assert!(
            is_uuid(&id),
            "case {label}: the minted id must be a lowercase uuid, got {id:?}"
        );

        let read = get_group(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label}: expected GetDataflowEndpointGroup 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupId"),
            id,
            "case {label}: the group must read back under the id it was created with, got {}",
            read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupArn"),
            expected_arn(&id),
            "case {label}: wrong ARN, got {}",
            read.body
        );
        assert_eq!(
            read.field("contactPrePassDurationSeconds"),
            &json!(120),
            "case {label}: wrong contactPrePassDurationSeconds, got {}",
            read.body
        );
        assert_eq!(
            read.field("contactPostPassDurationSeconds"),
            &json!(180),
            "case {label}: wrong contactPostPassDurationSeconds, got {}",
            read.body
        );
        assert_eq!(
            read.field("tags"),
            &json!({"stage": "created"}),
            "case {label}: wrong tags, got {}",
            read.body
        );

        let endpoints = read
            .field("endpointsDetails")
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: endpointsDetails is not a list: {}", read.body))
            .clone();
        assert_eq!(
            endpoints.len(),
            1,
            "case {label}: the group holds the one endpoint it was created with, got {:?}",
            endpoints
        );
        let endpoint = &endpoints[0];
        assert_eq!(
            endpoint.get("securityDetails"),
            Some(&security_details()),
            "case {label}: the endpoint must report the security details it was created \
             with, got {endpoint}"
        );
        assert_eq!(
            endpoint.get(member),
            Some(&with_member(&body, derived.0, &derived.1)),
            "case {label}: the endpoint must read back as it was created, plus the \
             service's own {}, got {endpoint}",
            derived.0
        );
        // `healthStatus`/`healthReasons` are the service's to report, not the caller's.
        assert_eq!(
            endpoint.get("healthStatus").and_then(Value::as_str),
            Some(health_status),
            "case {label}: wrong healthStatus, got {endpoint}"
        );
        assert_eq!(
            endpoint.get("healthReasons"),
            Some(&json!([health_reason])),
            "case {label}: wrong healthReasons, got {endpoint}"
        );
    }
}

#[tokio::test]
async fn the_health_a_request_asks_for_is_ignored_and_the_observed_health_reported() {
    let router = harness();

    // Every row asks for a health the endpoint does NOT have — the opposite of the health
    // the service observes for that kind — and the model documents both members as "ignored
    // when calling CreateDataflowEndpointGroup".
    for kind in endpoint_kinds() {
        let asked = if kind.health_status == HEALTH_HEALTHY {
            (HEALTH_UNHEALTHY, REASON_DATAPLANE_FAILURE)
        } else {
            (HEALTH_HEALTHY, REASON_HEALTHY)
        };
        let mut item = details(kind.member, &kind.body, true);
        let object = item.as_object_mut().expect("an endpoint details object");
        object.insert("healthStatus".to_string(), json!(asked.0));
        object.insert("healthReasons".to_string(), json!([asked.1]));

        let created = create_group(&router, json!({"endpointDetails": [item]})).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} {}",
            kind.label, created.status, created.body
        );
        let read = get_group(&router, &created.string("dataflowEndpointGroupId")).await;
        let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
        assert_eq!(
            endpoint.get("healthStatus").and_then(Value::as_str),
            Some(kind.health_status),
            "case {}: the requested healthStatus must be ignored, got {endpoint}",
            kind.label
        );
        assert_eq!(
            endpoint.get("healthReasons"),
            Some(&json!([kind.health_reason])),
            "case {}: the requested healthReasons must be ignored, got {endpoint}",
            kind.label
        );
    }
}

#[tokio::test]
async fn security_details_are_demanded_only_of_the_endpoint_kind_whose_streams_need_them() {
    let router = harness();

    // One row per endpoint member of `EndpointDetails`, each created with NO
    // `securityDetails`: the member is optional in the model, and the subnet/security
    // group/role triple it carries is the elastic-network-interface placement Ground
    // Station performs for a `DataflowEndpoint` only — an AWS Ground Station Agent
    // endpoint's stream is carried by the agent on the customer's own instance, so it is
    // described by `name` + `egressAddress` + `ingressAddress` alone.
    let mut accepted_ids = Vec::new();
    for kind in endpoint_kinds() {
        let reply = create_group(
            &router,
            json!({"endpointDetails": [details(kind.member, &kind.body, false)]}),
        )
        .await;

        if kind.needs_security_details {
            assert_eq!(
                reply.status, 431,
                "case {}: expected 431, got {} {}",
                kind.label, reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {}: wrong error shape, got {}",
                kind.label,
                reply.body
            );
            assert_eq!(
                reply.field("parameterName"),
                &json!("securityDetails"),
                "case {}: the refusal must name the missing member, got {}",
                kind.label,
                reply.body
            );
            assert!(
                reply.string("message").contains("securityDetails"),
                "case {}: the refusal must say what is missing, got {}",
                kind.label,
                reply.body
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {}: an endpoint whose streams need no VPC placement is created without \
             securityDetails, got {} {}",
            kind.label, reply.status, reply.body
        );
        let id = reply.string("dataflowEndpointGroupId");
        assert!(
            is_uuid(&id),
            "case {}: the minted id must be a lowercase uuid, got {id:?}",
            kind.label
        );

        // The group is read back from state: the endpoint it holds is the one the request
        // described, still carrying no `securityDetails`, plus the members the service
        // owns.
        let read = get_group(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetDataflowEndpointGroup 200, got {} {}",
            kind.label, read.status, read.body
        );
        let endpoints = read.field("endpointsDetails").as_array().unwrap_or_else(|| {
            panic!(
                "case {}: endpointsDetails is not a list: {}",
                kind.label, read.body
            )
        });
        assert_eq!(
            endpoints.len(),
            1,
            "case {}: the group holds the one endpoint it was created with, got {:?}",
            kind.label,
            endpoints
        );
        let endpoint = &endpoints[0];
        assert!(
            endpoint.get("securityDetails").is_none_or(Value::is_null),
            "case {}: a group created without securityDetails must report none, got \
             {endpoint}",
            kind.label
        );
        assert_eq!(
            endpoint.get(kind.member),
            Some(&with_member(&kind.body, kind.derived.0, &kind.derived.1)),
            "case {}: the endpoint must read back as it was created, plus the service's \
             own {}, got {endpoint}",
            kind.label,
            kind.derived.0
        );
        assert_eq!(
            endpoint.get("healthStatus").and_then(Value::as_str),
            Some(kind.health_status),
            "case {}: wrong healthStatus, got {endpoint}",
            kind.label
        );
        assert_eq!(
            endpoint.get("healthReasons"),
            Some(&json!([kind.health_reason])),
            "case {}: wrong healthReasons, got {endpoint}",
            kind.label
        );
        accepted_ids.push(id);
    }

    // Only the accepted creates are stored: the refused one left nothing behind.
    let mut listed = listed_ids(&router, None, accepted_ids.len()).await;
    listed.sort();
    accepted_ids.sort();
    assert_eq!(
        listed, accepted_ids,
        "only the groups that were accepted may be stored: expected {accepted_ids:?}, \
         got {listed:?}"
    );
}

#[tokio::test]
async fn an_agent_endpoint_group_without_security_details_mixes_no_better_than_one_with() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];
    let agent = &kinds[1];

    // Dropping `securityDetails` from the agent item changes nothing about the
    // same-type rule: a group still may not mix the two kinds, and the refusal is about
    // `endpointDetails`, not the absent member.
    let reply = create_group(
        &router,
        json!({"endpointDetails": [
            details(agent.member, &agent.body, false),
            details(dataflow.member, &dataflow.body, true),
        ]}),
    )
    .await;
    assert_eq!(
        reply.status, 431,
        "case an agent endpoint without securityDetails then a dataflow endpoint: \
         expected 431, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!("endpointDetails"),
        "case a mixed group: the refusal must name the member it is about, got {}",
        reply.body
    );

    let listed = listed_ids(&router, None, 0).await;
    assert!(
        listed.is_empty(),
        "a refused create must store nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn one_group_holds_endpoints_of_one_type_and_each_item_names_exactly_one() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];
    let agent = &kinds[1];
    let uplink_agent = &kinds[2];
    let downlink_agent = &kinds[3];

    // The rule under test is the operation's own documentation: "All dataflow endpoints
    // within a single dataflow endpoint group must be of the same type. You cannot mix
    // AWS Ground Station Agent endpoints with Dataflow endpoints in the same group" —
    // plus the one `EndpointDetails` describes ONE endpoint.
    let accepted: Vec<(&str, Vec<Value>)> = vec![
        (
            // Each endpoint carries a name of its own: a mission profile's
            // `dataflowEndpointConfig` selects an endpoint of a group by name, so the two
            // endpoints of one group differ in it (asserted in
            // `crate::dataflow_endpoint_addressing`).
            "two dataflow endpoints",
            vec![
                details(dataflow.member, &dataflow.body, true),
                details(
                    dataflow.member,
                    &with_member(&dataflow.body, "name", &json!("second-endpoint")),
                    true,
                ),
            ],
        ),
        (
            "two agent endpoints of different agent members",
            vec![
                details(agent.member, &agent.body, true),
                details(uplink_agent.member, &uplink_agent.body, true),
            ],
        ),
        (
            "three agent endpoints",
            vec![
                details(agent.member, &agent.body, true),
                details(uplink_agent.member, &uplink_agent.body, true),
                details(downlink_agent.member, &downlink_agent.body, true),
            ],
        ),
        ("a group with no endpoint at all", vec![]),
    ];
    let refused: Vec<(&str, Vec<Value>)> = vec![
        (
            "a dataflow endpoint then an agent endpoint",
            vec![
                details(dataflow.member, &dataflow.body, true),
                details(agent.member, &agent.body, true),
            ],
        ),
        (
            "an agent endpoint then a dataflow endpoint",
            vec![
                details(agent.member, &agent.body, true),
                details(dataflow.member, &dataflow.body, true),
            ],
        ),
        (
            "a downlink agent endpoint then a dataflow endpoint",
            vec![
                details(downlink_agent.member, &downlink_agent.body, true),
                details(dataflow.member, &dataflow.body, true),
            ],
        ),
        (
            "an item that names no endpoint",
            vec![json!({"securityDetails": security_details()})],
        ),
        (
            "an item that names two endpoints",
            vec![json!({
                "securityDetails": security_details(),
                "endpoint": dataflow.body.clone(),
                "awsGroundStationAgentEndpoint": agent.body.clone(),
            })],
        ),
        (
            "an item that names two agent endpoints",
            vec![json!({
                "securityDetails": security_details(),
                "uplinkAwsGroundStationAgentEndpoint": uplink_agent.body.clone(),
                "downlinkAwsGroundStationAgentEndpoint": downlink_agent.body.clone(),
            })],
        ),
    ];

    let mut created_ids = Vec::new();
    for (label, endpoints) in &accepted {
        let expected_endpoints = endpoints.len();
        let reply = create_group(&router, json!({"endpointDetails": endpoints})).await;
        assert_eq!(
            reply.status, 200,
            "case {label}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let id = reply.string("dataflowEndpointGroupId");
        let read = get_group(&router, &id).await;
        assert_eq!(
            read.field("endpointsDetails")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            expected_endpoints,
            "case {label}: the group must hold every endpoint it was created with, got {}",
            read.body
        );
        created_ids.push(id);
    }

    for (label, endpoints) in &refused {
        let reply = create_group(&router, json!({"endpointDetails": endpoints})).await;
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
            &json!("endpointDetails"),
            "case {label}: the refusal must name the member it is about, got {}",
            reply.body
        );
    }

    let mut stored = listed_ids(&router, None, created_ids.len()).await;
    stored.sort();
    created_ids.sort();
    assert_eq!(
        stored, created_ids,
        "only the accepted groups may be stored: expected {created_ids:?}, got {stored:?}"
    );
}

#[tokio::test]
async fn a_dataflow_endpoint_is_created_attached_and_an_agent_endpoint_waits_for_its_agent() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];
    let agent = &kinds[1];

    let dataflow_id = create_group(
        &router,
        json!({"endpointDetails": [details(dataflow.member, &dataflow.body, true)]}),
    )
    .await
    .string("dataflowEndpointGroupId");
    let agent_id = create_group(
        &router,
        json!({"endpointDetails": [details(agent.member, &agent.body, true)]}),
    )
    .await
    .string("dataflowEndpointGroupId");

    // The read taken straight after the create reports the endpoint ATTACHED: the recorded
    // group lifecycle creates a group and reads it back, and that read is answered
    // `created` / HEALTHY / [HEALTHY], so the group is handed back ready rather than in a
    // transient state the caller has to wait out.
    let read = get_group(&router, &dataflow_id).await;
    let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
    assert_eq!(
        endpoint.pointer("/endpoint/status").and_then(Value::as_str),
        Some(ENDPOINT_CREATED),
        "case straight after the create: expected {ENDPOINT_CREATED}, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthStatus").and_then(Value::as_str),
        Some(HEALTH_HEALTHY),
        "case straight after the create: an attached endpoint is healthy, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([REASON_HEALTHY])),
        "case straight after the create: wrong healthReasons, got {endpoint}"
    );

    std::thread::sleep(AFTER_SETTLE);

    // And it stays exactly that, however long the clock runs and however often it is read:
    // a resource handed back ready owes no later move, so no read's answer differs from the
    // first one's.
    let later = get_group(&router, &dataflow_id).await;
    assert_eq!(
        later.field("endpointsDetails"),
        read.field("endpointsDetails"),
        "case read again later: an attached endpoint must read the same, got {} then {}",
        read.body,
        later.body
    );
    let again = get_group(&router, &dataflow_id).await;
    assert_eq!(
        again.field("endpointsDetails"),
        read.field("endpointsDetails"),
        "case read twice: a read must not change what the next one sees, got {} then {}",
        read.body,
        again.body
    );

    // An agent endpoint has no agent registered for it, and no clock makes one register.
    let read = get_group(&router, &agent_id).await;
    let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
    assert_eq!(
        endpoint.get("healthStatus").and_then(Value::as_str),
        Some(HEALTH_UNHEALTHY),
        "case an agent endpoint with no agent: expected {HEALTH_UNHEALTHY}, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([REASON_NO_REGISTERED_AGENT])),
        "case an agent endpoint with no agent: no agent has registered, got {endpoint}"
    );
    assert_eq!(
        endpoint
            .pointer("/awsGroundStationAgentEndpoint/agentStatus")
            .and_then(Value::as_str),
        Some(AGENT_INACTIVE),
        "case an agent endpoint with no agent: expected {AGENT_INACTIVE}, got {endpoint}"
    );
}

#[tokio::test]
async fn the_pre_and_post_pass_durations_round_trip_inside_their_published_bounds() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];
    let endpoints = vec![details(dataflow.member, &dataflow.body, true)];

    // Each bound of the modelled 30..480 range, a value inside it, and one past each
    // bound; a value past a bound fails the model's own rules, which this service answers
    // with HTTP 400 `BadRequestException` and its one fixed sentence.
    let cases: Vec<(&str, Option<i64>, Option<i64>, bool)> = vec![
        ("no duration at all", None, None, true),
        ("the published minimum", Some(DURATION_MIN), Some(DURATION_MIN), true),
        ("the published maximum", Some(DURATION_MAX), Some(DURATION_MAX), true),
        ("one inside the range", Some(120), Some(240), true),
        ("one second under the minimum", Some(DURATION_MIN - 1), None, false),
        ("one second over the maximum", None, Some(DURATION_MAX + 1), false),
    ];

    for (label, pre, post, accepted) in &cases {
        let mut request = Map::new();
        request.insert("endpointDetails".to_string(), json!(endpoints));
        if let Some(pre) = pre {
            request.insert("contactPrePassDurationSeconds".to_string(), json!(pre));
        }
        if let Some(post) = post {
            request.insert("contactPostPassDurationSeconds".to_string(), json!(post));
        }

        let reply = create_group(&router, Value::Object(request)).await;
        if !accepted {
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
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {label}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let read = get_group(&router, &reply.string("dataflowEndpointGroupId")).await;
        assert_eq!(
            read.field("contactPrePassDurationSeconds"),
            &pre.map_or(Value::Null, |value| json!(value)),
            "case {label}: wrong contactPrePassDurationSeconds, got {}",
            read.body
        );
        assert_eq!(
            read.field("contactPostPassDurationSeconds"),
            &post.map_or(Value::Null, |value| json!(value)),
            "case {label}: wrong contactPostPassDurationSeconds, got {}",
            read.body
        );
        // A group carries a tag map even when it was created without one.
        assert_eq!(
            read.field("tags"),
            &json!({}),
            "case {label}: tags must serialize as a map, got {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_listing_returns_every_group_once_and_a_deleted_group_is_gone() {
    let router = harness();
    let kinds = endpoint_kinds();
    let dataflow = &kinds[0];

    /// How many groups this suite creates, and the page sizes it reads them back in.
    const GROUPS: usize = 5;

    let mut created = Vec::new();
    for index in 0..GROUPS {
        let mut endpoint = dataflow.body.as_object().expect("an object").clone();
        endpoint.insert("name".to_string(), json!(format!("endpoint-{index}")));
        let reply = create_group(
            &router,
            json!({"endpointDetails": [details(dataflow.member, &Value::Object(endpoint), true)]}),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case group {index}: expected 200, got {} {}",
            reply.status, reply.body
        );
        created.push(reply.string("dataflowEndpointGroupId"));
    }
    created.sort();

    // Every page size reports the same groups, each exactly once, in one stable order.
    // The page walk itself (with the per-page bound and token-shape assertions it carries)
    // lives in the listing suite, so this target holds one walker rather than two.
    for page_size in [None, Some(1), Some(2), Some(GROUPS as i64), Some(100)] {
        let listed = listed_ids(&router, page_size, GROUPS).await;
        assert_eq!(
            listed, created,
            "case maxResults={page_size:?}: expected {created:?}, got {listed:?}"
        );
    }

    // A delete removes the group: every later read of it answers 434, as does repeating
    // the delete, and the listing no longer carries it.
    let removed = created.remove(2);
    let deleted = delete_group(&router, &removed).await;
    assert_eq!(
        deleted.status, 200,
        "case delete: expected 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("dataflowEndpointGroupId"),
        removed,
        "case delete: it must answer the group it removed, got {}",
        deleted.body
    );

    for (label, reply) in [
        ("a read of a deleted group", get_group(&router, &removed).await),
        ("the same delete repeated", delete_group(&router, &removed).await),
        ("a read of a group never created", get_group(&router, ABSENT_UUID).await),
    ] {
        // The not-found shape has ONE owner in this target: the deletion suite's helper.
        assert_group_absent(label, &reply);
    }

    let listed = listed_ids(&router, Some(2), created.len()).await;
    assert_eq!(
        listed, created,
        "after the delete the listing must report the remaining groups: expected \
         {created:?}, got {listed:?}"
    );

    // A token this listing did not issue is refused rather than resumed at an offset into
    // a different sequence.
    let refused = list_groups(&router, "?nextToken=missionprofiles.2").await;
    assert_eq!(
        refused.status, 431,
        "case a foreign nextToken: expected 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.field("parameterName"),
        &json!("nextToken"),
        "case a foreign nextToken: the refusal must name the parameter, got {}",
        refused.body
    );
}
