// SPDX-License-Identifier: CC0-1.0

//! The MTU a dataflow endpoint group's endpoints are framed at, over both creates.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded call: every
//! `mtu` reachable from an endpoint of a dataflow endpoint group is the "Maximum
//! transmission unit (MTU) size in bytes of a dataflow endpoint", one documented member of
//! one documented range of 1400..1500, so a create naming a size outside it is refused
//! whichever member carried it and nothing is stored — while an omitted `mtu`, the member
//! being optional, is accepted.
//!
//! The range reaches two of the three `mtu` members through the model itself
//! (`DataflowEndpoint$mtu` and `RangedConnectionDetails$mtu` carry `range 1400..1500`), so
//! those are refused by the request validator; `ConnectionDetails$mtu` carries the same
//! documentation and no `range` of its own, so there the service applies the bound and
//! answers its own modelled refusal naming the member at fault.
//!
//! Every accepted row is read back through `GetDataflowEndpointGroup`, so an accepted size
//! is asserted from state rather than from the create's answer alone.

use serde_json::{json, Map, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    BAD_REQUEST_ERROR_CODE, INVALID_REQUEST_BODY, MTU_MAX, MTU_MIN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_listing::listed_ids;
use crate::dataflow_endpoint_groups::{create_group, details, endpoint_kinds, get_group};
use crate::scheduling_loop::{harness, send, Reply};

/// What applies the documented 1400..1500 range to a given `mtu` member, which is what
/// decides the refusal a request outside it hears.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BoundedBy {
    /// The member's own shape carries `range 1400..1500`, so the request validator refuses
    /// a value outside it before any handler runs.
    Model,
    /// The member's shape carries no `range`, so the service holds it to the same bound as
    /// its siblings and refuses with its own modelled error.
    Handler,
}

/// One `mtu`-valued member reachable from the endpoint `member` describes: the JSON pointer
/// into that member's body, and what bounds the value there.
///
/// Source: `smithy_query(query="closure", operation="CreateDataflowEndpointGroup")` with
/// `smithy_query(query="shape", shape="ConnectionDetails,RangedConnectionDetails,
/// DataflowEndpoint")` — `DataflowEndpoint$mtu` and `RangedConnectionDetails$mtu` carry
/// `range {min: 1400, max: 1500}`, `ConnectionDetails$mtu` carries none. An agent endpoint
/// pairs an `egressAddress` (`ConnectionDetails`) with an `ingressAddress`
/// (`RangedConnectionDetails`); an uplink endpoint's `dataflowDetails` pairs
/// `ingressAddressAndPort` (`ConnectionDetails`) with `agentIpAndPortAddress`
/// (`RangedConnectionDetails`), and a downlink endpoint's pairs `egressAddressAndPort`
/// (`ConnectionDetails`) with the same `agentIpAndPortAddress`.
fn mtu_members(member: &str) -> Vec<(&'static str, BoundedBy)> {
    match member {
        "endpoint" => vec![("/mtu", BoundedBy::Model)],
        "awsGroundStationAgentEndpoint" => vec![
            ("/egressAddress/mtu", BoundedBy::Handler),
            ("/ingressAddress/mtu", BoundedBy::Model),
        ],
        "uplinkAwsGroundStationAgentEndpoint" => vec![
            (
                "/dataflowDetails/agentConnectionDetails/ingressAddressAndPort/mtu",
                BoundedBy::Handler,
            ),
            (
                "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/mtu",
                BoundedBy::Model,
            ),
        ],
        "downlinkAwsGroundStationAgentEndpoint" => vec![
            (
                "/dataflowDetails/agentConnectionDetails/egressAddressAndPort/mtu",
                BoundedBy::Handler,
            ),
            (
                "/dataflowDetails/agentConnectionDetails/agentIpAndPortAddress/mtu",
                BoundedBy::Model,
            ),
        ],
        other => panic!("no mtu members are known for the endpoint member {other}"),
    }
}

/// The sizes inside the documented range, each bound included, and the absent member the
/// model leaves optional — one row each, so a row varies the MTU and nothing else.
fn accepted_sizes() -> Vec<Option<i32>> {
    vec![
        Some(MTU_MIN),
        Some(MTU_MIN + 1),
        Some((MTU_MIN + MTU_MAX) / 2),
        Some(MTU_MAX),
        None,
    ]
}

/// The sizes outside it: one byte under the first, one over the last, and sizes no MTU can
/// be framed at at all.
fn refused_sizes() -> Vec<i32> {
    vec![MTU_MIN - 1, MTU_MAX + 1, 0, -1, 9000]
}

/// `body` with the `mtu` at `pointer` set to `mtu`, or removed when it is `None`.
fn with_mtu(body: &Value, pointer: &str, mtu: Option<i32>) -> Value {
    let mut body = body.clone();
    let (parent, leaf) = pointer
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("{pointer} is not a pointer to a member"));
    let object = body
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("the endpoint body has no object at {parent:?}"));
    match mtu {
        Some(mtu) => {
            object.insert(leaf.to_string(), json!(mtu));
        }
        None => {
            object.remove(leaf);
        }
    }
    body
}

/// The member path a refusal names for the value at `pointer` of the endpoint `member`
/// carried by the list item `item`.
fn parameter_path(item: &str, member: &str, pointer: &str) -> String {
    format!(
        "{item}.{member}.{}",
        pointer.trim_start_matches('/').replace('/', ".")
    )
}

/// Assert `reply` refuses a size outside the range the way `bounded_by` decides: the
/// service's own modelled `InvalidParameterException` (HTTP 431) naming `parameter`, the
/// size and both bounds, or the request validator's one fixed sentence (HTTP 400).
fn assert_refused(label: &str, reply: &Reply, bounded_by: BoundedBy, parameter: &str, mtu: i32) {
    match bounded_by {
        BoundedBy::Handler => {
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
            let message = reply.string("message");
            assert!(
                message.contains(&mtu.to_string())
                    && message.contains(&MTU_MIN.to_string())
                    && message.contains(&MTU_MAX.to_string()),
                "case {label}: the refusal must say which size and which bounds, got \
                 {message:?}"
            );
        }
        BoundedBy::Model => {
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
    }
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

/// `POST /dataflowEndpointGroupV2`.
async fn create_group_v2(router: &MultiServiceRouter, body: Value) -> Reply {
    send(router, "POST", "/dataflowEndpointGroupV2", Some(body)).await
}

#[tokio::test]
async fn every_mtu_an_endpoint_is_framed_at_must_be_inside_the_documented_range() {
    let router = harness();

    let mut created: Vec<String> = Vec::new();
    let mut handler_bounded_rows = 0;
    for kind in endpoint_kinds() {
        for (pointer, bounded_by) in mtu_members(kind.member) {
            if bounded_by == BoundedBy::Handler {
                handler_bounded_rows += 1;
            }

            for mtu in accepted_sizes() {
                let label = format!("{} at {pointer} = {mtu:?}", kind.label);
                let body = with_mtu(&kind.body, pointer, mtu);
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                assert_eq!(
                    reply.status, 200,
                    "case {label}: a size inside the documented range is accepted, got {} {}",
                    reply.status, reply.body
                );

                // Read back from state: the endpoint the group holds is framed at the size
                // the request described, and carries no size it did not.
                let id = reply.string("dataflowEndpointGroupId");
                let stored = stored_endpoint(&router, &id, kind.member).await;
                assert_eq!(
                    stored.pointer(pointer),
                    mtu.map(|mtu| json!(mtu)).as_ref(),
                    "case {label}: the stored endpoint must report the mtu it was created \
                     with, got {stored}"
                );
                created.push(id);
            }

            for mtu in refused_sizes() {
                let label = format!("{} at {pointer} = {mtu}", kind.label);
                let body = with_mtu(&kind.body, pointer, Some(mtu));
                let reply = create_group(
                    &router,
                    json!({"endpointDetails": [details(kind.member, &body, true)]}),
                )
                .await;
                assert_refused(
                    &label,
                    &reply,
                    bounded_by,
                    &parameter_path("endpointDetails[0]", kind.member, pointer),
                    mtu,
                );
            }
        }
    }

    // The three agent connection pairs each carry one `ConnectionDetails`, the member no
    // model range reaches, so the service itself answered three rows' worth of refusals.
    assert_eq!(
        handler_bounded_rows, 3,
        "the three agent endpoint members each carry one unbounded mtu, got \
         {handler_bounded_rows}"
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
async fn the_v2_create_holds_its_endpoints_to_the_same_mtu_range() {
    let router = harness();

    // `CreateEndpointDetails` carries agent endpoints only — the uplink and downlink
    // members (`smithy_query(query="shape", shape="CreateEndpointDetails")`) — each
    // describing its endpoint with the same shapes the first create takes, so the rows come
    // from the same table.
    let kinds: Vec<_> = endpoint_kinds()
        .into_iter()
        .filter(|kind| {
            matches!(
                kind.member,
                "uplinkAwsGroundStationAgentEndpoint" | "downlinkAwsGroundStationAgentEndpoint"
            )
        })
        .collect();
    assert_eq!(
        kinds.len(),
        2,
        "this create takes the two agent endpoint members, got {}",
        kinds.len()
    );

    let mut created: Vec<String> = Vec::new();
    for kind in &kinds {
        for (pointer, bounded_by) in mtu_members(kind.member) {
            for mtu in accepted_sizes() {
                let label = format!("{} at {pointer} = {mtu:?}", kind.label);
                let body = with_mtu(&kind.body, pointer, mtu);
                let mut endpoint = Map::new();
                endpoint.insert(kind.member.to_string(), body);
                let reply =
                    create_group_v2(&router, json!({"endpoints": [Value::Object(endpoint)]})).await;
                assert_eq!(
                    reply.status, 200,
                    "case {label}: a size inside the documented range is accepted, got {} {}",
                    reply.status, reply.body
                );

                let id = reply.string("dataflowEndpointGroupId");
                let stored = stored_endpoint(&router, &id, kind.member).await;
                assert_eq!(
                    stored.pointer(pointer),
                    mtu.map(|mtu| json!(mtu)).as_ref(),
                    "case {label}: the stored endpoint must report the mtu it was created \
                     with, got {stored}"
                );
                created.push(id);
            }

            for mtu in refused_sizes() {
                let label = format!("{} at {pointer} = {mtu}", kind.label);
                let body = with_mtu(&kind.body, pointer, Some(mtu));
                let mut endpoint = Map::new();
                endpoint.insert(kind.member.to_string(), body);
                let reply =
                    create_group_v2(&router, json!({"endpoints": [Value::Object(endpoint)]})).await;
                assert_refused(
                    &label,
                    &reply,
                    bounded_by,
                    &parameter_path("endpoints[0]", kind.member, pointer),
                    mtu,
                );
            }
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
