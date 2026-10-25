// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness this target's suites share.
//!
//! The definitions live in [`crate::network_and_node_lifecycle`], the suite that grew
//! them first; this module re-exports them (and adds the engine-keeping router every
//! suite that reads the store needs, the id-addressed reads and the listing projection
//! more than one suite asserts over), so a suite can reach the harness without importing
//! from another suite's module.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

pub(crate) use crate::network_and_node_lifecycle::{
    available_network, call, call_in_region, create_network, create_network_body, is_date_time,
    is_resource_id, member_configuration, new_router, router_over, Answer, ACCOUNT,
    PAST_TRANSITION, REGION,
};

use crate::list_members::{seed_members, Seed};

/// A router and the store its handlers read and write — the SAME engine, so a suite can
/// reach the records the handlers answer from.
pub(crate) fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// One `DeleteMember` call: `DELETE /networks/{NetworkId}/members/{MemberId}`.
pub(crate) async fn delete_member(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/networks/{network_id}/members/{member_id}"),
        None,
    )
    .await
}

/// One `GetMember` call.
pub(crate) async fn get_member(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/members/{member_id}"),
        None,
    )
    .await
}

/// One `DeleteNode` call: `DELETE /networks/{NetworkId}/nodes/{NodeId}?memberId=…`, the
/// member scope every node of a Hyperledger Fabric network is addressed through.
pub(crate) async fn delete_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await
}

/// One `GetNetwork` call.
pub(crate) async fn get_network(router: &MultiServiceRouter, network_id: &str) -> Answer {
    call(router, "GET", &format!("/networks/{network_id}"), None).await
}

/// One `GetNode` call, scoped by the member that owns the node.
pub(crate) async fn get_node(
    router: &MultiServiceRouter,
    network_id: &str,
    member_id: &str,
    node_id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/networks/{network_id}/nodes/{node_id}?memberId={member_id}"),
        None,
    )
    .await
}

/// The `Id` of every record the answer reports under `member` (`Members`, `Nodes`,
/// `Networks`, `Accessors`, …), in the order it reported them — which is the order the
/// listing pages on.
pub(crate) fn listed_ids(answer: &Answer, member: &str) -> Vec<String> {
    answer.body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The member ids `ListMembers` reports for `network_id` under `query` (empty for none).
pub(crate) async fn listed_member_ids(
    router: &MultiServiceRouter,
    network_id: &str,
    query: &str,
) -> Vec<String> {
    let path = if query.is_empty() {
        format!("/networks/{network_id}/members")
    } else {
        format!("/networks/{network_id}/members?{query}")
    };
    listed_ids(&call(router, "GET", &path, None).await, "Members")
}

/// A network that has settled to `AVAILABLE`, holding its founding member plus `extra`
/// seeded members this account owns and the clock owes nothing. Answers the router, the
/// network id and every member id on it, in the stored (id) order the listings page on.
///
/// Each seeded record is a copy of the one `CreateNetwork` stored, with only its name,
/// status and owning account changed, so a case exercises the handlers rather than a
/// record no create could have produced.
pub(crate) async fn network_with_members(
    token: &str,
    name: &str,
    extra: usize,
) -> (MultiServiceRouter, String, Vec<String>) {
    let (router, engine) = router_and_engine();
    let (network_id, founder) = create_network(&router, create_network_body(token, name)).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let seeds: Vec<Seed> = (0..extra)
        .map(|_| Seed {
            name: "seeded-org",
            status: "AVAILABLE",
            account: ACCOUNT,
        })
        .collect();
    let mut members = seed_members(&engine, &network_id, &seeds);
    members.push(founder);
    members.sort();
    (router, network_id, members)
}
