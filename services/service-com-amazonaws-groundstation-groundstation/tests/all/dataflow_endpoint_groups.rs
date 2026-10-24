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
//! * every endpoint of a group needs the `securityDetails` its streams are connected
//!   with, each `EndpointDetails` names exactly one endpoint, and one group may not mix
//!   AWS Ground Station Agent endpoints with dataflow endpoints — each refused with
//!   `InvalidParameterException` (HTTP 431), and nothing stored;
//! * a dataflow endpoint is admitted `creating` and becomes `created` when the clock
//!   makes that move due, while an agent endpoint no agent has registered for stays
//!   `NO_REGISTERED_AGENT`;
//! * the pre/post-pass durations round-trip at each published bound and a value past
//!   either one is refused by the model's own rules;
//! * `ListDataflowEndpointGroups` returns every group exactly once across its pages, and
//!   a deleted group is gone from every read.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    AGENT_INACTIVE, BAD_REQUEST_ERROR_CODE, DATAFLOW_ENDPOINT_GROUP_NOT_FOUND, ENDPOINT_CREATED,
    ENDPOINT_CREATING, HEALTH_HEALTHY, HEALTH_UNHEALTHY, INVALID_REQUEST_BODY,
    REASON_HEALTHY, REASON_INITIALIZING_DATAPLANE, REASON_NO_REGISTERED_AGENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{harness, is_uuid, send, Reply, ABSENT_UUID, REGION};

/// Longer than this service's chosen `creating` -> `created` endpoint settle span, so a
/// read taken after it observes the move the service owed.
///
/// Waited out with `std::thread::sleep` rather than `tokio::time::sleep`: the emulator
/// stores a due instant and never arms a timer, so there is no task to drive while
/// waiting, and this workspace's `tokio` carries no `time` feature.
const AFTER_SETTLE: Duration = Duration::from_millis(600);

/// The published bounds of `DataflowEndpointGroupDurationInSeconds` (range 30..480 —
/// `smithy_query(query="shape", shape="DataflowEndpointGroupDurationInSeconds")`).
const DURATION_MIN: i64 = 30;
/// See [`DURATION_MIN`].
const DURATION_MAX: i64 = 480;

/// One row of the endpoint-kind table: a member of `EndpointDetails` that describes an
/// endpoint, the value a request carries in it, and what the SERVICE adds to it.
struct Kind {
    /// Names the row in every assertion.
    label: &'static str,
    /// The modelled member name, as the wire spells it.
    member: &'static str,
    /// The member's value as the request carries it.
    body: Value,
    /// The member the service fills in on the endpoint itself, and the value it reports.
    derived: (&'static str, Value),
    /// The one `CapabilityHealthReason` the service reports for a freshly created
    /// endpoint of this kind.
    health_reason: &'static str,
}

/// The four endpoint members of `EndpointDetails`, one row each.
///
/// Source: `smithy_query(query="shape", shape="EndpointDetails")` — `endpoint`,
/// `awsGroundStationAgentEndpoint`, `uplinkAwsGroundStationAgentEndpoint` and
/// `downlinkAwsGroundStationAgentEndpoint` — and the shape of each member's own target
/// (`smithy_query(query="closure", operation="CreateDataflowEndpointGroup")`).
fn endpoint_kinds() -> Vec<Kind> {
    vec![
        Kind {
            label: "a dataflow endpoint",
            member: "endpoint",
            body: json!({
                "name": "downlink-demod-decode",
                "address": {"name": "172.31.0.10", "port": 55888},
                "mtu": 1500,
            }),
            // A dataflow endpoint is admitted `creating`: the service has not finished
            // attaching it when the create answers.
            derived: ("status", json!(ENDPOINT_CREATING)),
            health_reason: REASON_INITIALIZING_DATAPLANE,
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
            health_reason: REASON_NO_REGISTERED_AGENT,
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
            health_reason: REASON_NO_REGISTERED_AGENT,
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
            health_reason: REASON_NO_REGISTERED_AGENT,
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
fn details(member: &str, body: &Value, with_security: bool) -> Value {
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
async fn create_group(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/dataflowEndpointGroup", Some(body)).await
}

/// `GET /dataflowEndpointGroup/{dataflowEndpointGroupId}`.
async fn get_group(router: &MultiServiceRouter, id: &str) -> Reply {
    send(router, "GET", &format!("/dataflowEndpointGroup/{id}"), None).await
}

/// `DELETE /dataflowEndpointGroup/{dataflowEndpointGroupId}`.
async fn delete_group(router: &MultiServiceRouter, id: &str) -> Reply {
    send(
        router,
        "DELETE",
        &format!("/dataflowEndpointGroup/{id}"),
        None,
    )
    .await
}

/// `GET /dataflowEndpointGroup`, with `query` appended verbatim (`""` for no filters).
async fn list_groups(router: &MultiServiceRouter, query: &str) -> Reply {
    send(router, "GET", &format!("/dataflowEndpointGroup{query}"), None).await
}

/// How many groups the listing reports, across however many pages it takes.
async fn list_all_ids(router: &MultiServiceRouter, page_size: Option<usize>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(next) = &token {
            query.push(format!("nextToken={next}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let page = list_groups(router, &suffix).await;
        assert_eq!(
            page.status, 200,
            "ListDataflowEndpointGroups{suffix}: expected 200, got {} {}",
            page.status, page.body
        );
        for item in page
            .field("dataflowEndpointGroupList")
            .as_array()
            .unwrap_or_else(|| panic!("dataflowEndpointGroupList is not a list: {}", page.body))
        {
            ids.push(
                item.get("dataflowEndpointGroupId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a list item without an id: {item}"))
                    .to_string(),
            );
        }
        match page.field("nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    ids
}

/// The ARN this service mints for a group: `dataflow-endpoint-group/<uuid>` under the
/// request's region and the emulator's account.
fn expected_arn(id: &str) -> String {
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
            health_reason,
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
            Some(HEALTH_UNHEALTHY),
            "case {label}: a freshly created endpoint is not healthy yet, got {endpoint}"
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

    // Every row asks for a health the endpoint does not have; the model documents both
    // members as "ignored when calling CreateDataflowEndpointGroup".
    for kind in endpoint_kinds() {
        let mut item = details(kind.member, &kind.body, true);
        let object = item.as_object_mut().expect("an endpoint details object");
        object.insert("healthStatus".to_string(), json!(HEALTH_HEALTHY));
        object.insert("healthReasons".to_string(), json!([REASON_HEALTHY]));

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
            Some(HEALTH_UNHEALTHY),
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
async fn an_endpoint_without_security_details_is_refused_whatever_kind_it_is() {
    let router = harness();

    for kind in endpoint_kinds() {
        let refused = create_group(
            &router,
            json!({"endpointDetails": [details(kind.member, &kind.body, false)]}),
        )
        .await;
        assert_eq!(
            refused.status, 431,
            "case {}: expected 431, got {} {}",
            kind.label, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {}: wrong error shape, got {}",
            kind.label,
            refused.body
        );
        assert_eq!(
            refused.field("parameterName"),
            &json!("securityDetails"),
            "case {}: the refusal must name the missing member, got {}",
            kind.label,
            refused.body
        );
        assert!(
            refused.string("message").contains("securityDetails"),
            "case {}: the refusal must say what is missing, got {}",
            kind.label,
            refused.body
        );
    }

    assert!(
        list_all_ids(&router, None).await.is_empty(),
        "a refused create must store nothing, got {:?}",
        list_all_ids(&router, None).await
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
            "two dataflow endpoints",
            vec![
                details(dataflow.member, &dataflow.body, true),
                details(dataflow.member, &dataflow.body, true),
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

    let mut stored = list_all_ids(&router, None).await;
    stored.sort();
    created_ids.sort();
    assert_eq!(
        stored, created_ids,
        "only the accepted groups may be stored: expected {created_ids:?}, got {stored:?}"
    );
}

#[tokio::test]
async fn a_dataflow_endpoint_settles_from_creating_to_created_and_an_agent_endpoint_waits_for_its_agent(
) {
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

    // Straight after the create, the endpoint is still being attached.
    let read = get_group(&router, &dataflow_id).await;
    let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
    assert_eq!(
        endpoint.pointer("/endpoint/status").and_then(Value::as_str),
        Some(ENDPOINT_CREATING),
        "case straight after the create: expected {ENDPOINT_CREATING}, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([REASON_INITIALIZING_DATAPLANE])),
        "case straight after the create: expected the dataplane to be initializing, got {endpoint}"
    );

    std::thread::sleep(AFTER_SETTLE);

    // Past the due instant the move the service owed is materialised by the read.
    let read = get_group(&router, &dataflow_id).await;
    let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
    assert_eq!(
        endpoint.pointer("/endpoint/status").and_then(Value::as_str),
        Some(ENDPOINT_CREATED),
        "case after the settle span: expected {ENDPOINT_CREATED}, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthStatus").and_then(Value::as_str),
        Some(HEALTH_HEALTHY),
        "case after the settle span: an attached endpoint is healthy, got {endpoint}"
    );
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([REASON_HEALTHY])),
        "case after the settle span: wrong healthReasons, got {endpoint}"
    );

    // The move is the clock's, so a read repeated after it reports the same thing.
    let again = get_group(&router, &dataflow_id).await;
    assert_eq!(
        again.field("endpointsDetails"),
        read.field("endpointsDetails"),
        "case read twice after the settle span: a read must not change what the next one \
         sees, got {} then {}",
        read.body,
        again.body
    );

    // An agent endpoint has no agent registered for it, and no clock makes one register.
    let read = get_group(&router, &agent_id).await;
    let endpoint = &read.field("endpointsDetails").as_array().expect("a list")[0];
    assert_eq!(
        endpoint.get("healthReasons"),
        Some(&json!([REASON_NO_REGISTERED_AGENT])),
        "case an agent endpoint after the settle span: no agent has registered, got {endpoint}"
    );
    assert_eq!(
        endpoint
            .pointer("/awsGroundStationAgentEndpoint/agentStatus")
            .and_then(Value::as_str),
        Some(AGENT_INACTIVE),
        "case an agent endpoint after the settle span: expected {AGENT_INACTIVE}, got {endpoint}"
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
    for page_size in [None, Some(1), Some(2), Some(GROUPS), Some(100)] {
        let listed = list_all_ids(&router, page_size).await;
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
        assert_eq!(
            reply.status, 434,
            "case {label}: expected 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {label}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            DATAFLOW_ENDPOINT_GROUP_NOT_FOUND,
            "case {label}: wrong refusal text, got {}",
            reply.body
        );
    }

    let listed = list_all_ids(&router, Some(2)).await;
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
