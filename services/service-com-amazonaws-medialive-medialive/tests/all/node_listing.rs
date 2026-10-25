// SPDX-License-Identifier: CC0-1.0

//! `ListNodes` — the node collection of ONE MediaLive Anywhere cluster — end to
//! end over the restJson1 wire.
//!
//! The rules the suites below assert, each over a TABLE that spans its class
//! rather than one recorded request:
//!
//! 1. The listing reports the nodes the store holds for the `ClusterId` the path
//!    names, each exactly once, in the order their ids were minted, cut into pages
//!    of at most `MaxResults` (bounded by the service's own ceiling,
//!    `MAX_LIST_PAGE_SIZE`) whose `NextToken` resumes at the first node not yet
//!    reported and is absent on the last page.
//! 2. Every member of a reported `DescribeNodeSummary` is the stored node read
//!    back — the same projection `DescribeNode` answers from — so a listed summary
//!    and a described node can never disagree, before a deregistration has come
//!    due or after; and a node is reported only through the cluster it was created
//!    in.
//! 3. A listing request the service cannot read (a `MaxResults` below 1, or a
//!    `ClusterId` naming no cluster) is refused with the modelled 400
//!    `BadRequestException` and changes nothing, a `NextToken` past the end of the
//!    collection reports an empty last page, and the listing itself is a READ: two
//!    consecutive walks report the same nodes.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::without_state;
// The node request helpers, the `NodeRole` and `NetworkInterfaceMode` value
// lists and the create/describe/delete calls are the node lifecycle suite's, so
// a listing and a describe of the same node are driven through ONE harness.
use crate::node_lifecycle::{
    cluster, created, delete_node, described, mapping, node_id, nodes_uri, INTERFACE_MODES, ROLES,
};
use crate::wire::{call, error_code, medialive};

/// `ListNodes` on `cluster_id`, with `query` appended verbatim (`MaxResults` and
/// `NextToken` are both `@httpQuery`).
async fn list(router: &MultiServiceRouter, cluster_id: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{}{query}", nodes_uri(cluster_id)),
        None,
    )
    .await
}

/// The `nodes` member as a list, whatever its length. An empty list and an absent
/// member both mean "no entries" to a client, and which of the two the restJson1
/// codec writes is the serializer's choice, so both read here as the empty list.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    match body.get("nodes") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a nodes list, got {other}"),
    }
}

/// Walk every page of one cluster's listing, answering the ids reported in order
/// and the size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    page_size: Option<i32>,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = String::new();
        if let Some(max_results) = page_size {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("maxResults={max_results}"));
        }
        if let Some(next) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={next}"));
        }

        let (status, body) = list(router, cluster_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListNodes to answer 200, got {status}: {body}"
        );
        let page = summaries(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} summaries in a page, got {}",
            page.len()
        );
        pages.push(page.len());
        for summary in &page {
            ids.push(node_id(case, summary));
        }

        token = body["nextToken"].as_str().map(ToString::to_string);
        // A token is handed out only when something is left over, so an empty page
        // can never carry one.
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        if token.is_none() {
            break;
        }
        assert!(
            pages.len() <= 256,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// One row of the pagination table: how many nodes the listed cluster holds, how
/// many a SECOND cluster holds beside it (so the walk is asserted to be scoped),
/// and the page size the request asks for.
struct Paging {
    case: &'static str,
    nodes: usize,
    bystanders: usize,
    page_size: Option<i32>,
}

/// Rule 1: the listing reports the nodes the store holds for the cluster the path
/// names, each exactly once, in the order their ids were minted, cut into pages of
/// at most `MaxResults` (bounded by the service's own ceiling) whose `NextToken`
/// resumes at the first node not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of nodes in the listed cluster, the count in a
/// second cluster whose nodes interleave with them in the one id space, and the
/// requested page size — including none stored at all (the stub issue's own
/// reproduction: a listing on a cluster no create has placed a node in), a page
/// size of one, a size that divides the collection unevenly, no size at all, a size
/// one past the collection and a size past the service's ceiling — because that is
/// what the rule generalises over.
#[tokio::test]
async fn the_listing_pages_through_every_node_of_the_cluster_in_mint_order() {
    let table = [
        Paging {
            case: "no nodes at all",
            nodes: 0,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "no nodes, another cluster has two",
            nodes: 0,
            bystanders: 2,
            page_size: Some(1),
        },
        Paging {
            case: "one node, page of one",
            nodes: 1,
            bystanders: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three nodes, page of one",
            nodes: 3,
            bystanders: 2,
            page_size: Some(1),
        },
        Paging {
            case: "five nodes, page of two",
            nodes: 5,
            bystanders: 3,
            page_size: Some(2),
        },
        Paging {
            case: "four nodes, default page size",
            nodes: 4,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "three nodes, page of four",
            nodes: 3,
            bystanders: 1,
            page_size: Some(4),
        },
        Paging {
            case: "three nodes, page size at the ceiling",
            nodes: 3,
            bystanders: 0,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32),
        },
        Paging {
            case: "two nodes, page size past the ceiling",
            nodes: 2,
            bystanders: 2,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        // More nodes than one page can carry whatever the request asks for, so the
        // ceiling itself is what cuts the pages.
        Paging {
            case: "past the ceiling, no page size",
            nodes: MAX_LIST_PAGE_SIZE + 3,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "past the ceiling, page size past the ceiling",
            nodes: MAX_LIST_PAGE_SIZE + 3,
            bystanders: 2,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 * 2),
        },
    ];

    for Paging {
        case,
        nodes,
        bystanders,
        page_size,
    } in table
    {
        let router = medialive();
        let listed_cluster = cluster(&router, "listed").await;
        let other_cluster = cluster(&router, "other").await;

        // The two clusters' nodes are created ALTERNATELY, so their ids interleave
        // in the one minted id space and a listing that ignored the cluster scope
        // could not answer the expected ids.
        let mut minted: Vec<String> = Vec::new();
        for index in 0..nodes.max(bystanders) {
            if index < nodes {
                let node = created(
                    &router,
                    case,
                    &listed_cluster,
                    json!({ "name": format!("listed-{index}") }),
                )
                .await;
                minted.push(node_id(case, &node));
            }
            if index < bystanders {
                created(
                    &router,
                    case,
                    &other_cluster,
                    json!({ "name": format!("other-{index}") }),
                )
                .await;
            }
        }

        let (reported, pages) = every_page(&router, case, &listed_cluster, page_size).await;
        assert_eq!(
            reported, minted,
            "case {case:?}: expected the listing to report every node of the cluster once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n` every page but
        // the last carries exactly `n`.
        let effective = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if nodes == 0 { 1 } else { nodes.div_ceil(effective) };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {nodes} nodes of {effective}, \
             got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                nodes - effective * index
            } else {
                effective
            };
            assert_eq!(
                *page, expected,
                "case {case:?}: expected page {index} to carry {expected} summaries, \
                 got {pages:?}"
            );
        }
    }
}

/// One row of the stored-node table: what the create request carries and whether
/// the node's delete is accepted before the listing runs.
struct Stored {
    case: &'static str,
    body: fn() -> Value,
    delete_it: bool,
    /// The `NodeState` the node reports straight after the create (or after the
    /// delete was accepted) and the one the clock owes it, from the model's own
    /// enum (`smithy_query(query="shape", shape="NodeState")`).
    state: &'static str,
    settled_state: &'static str,
}

/// Rule 2: every member of a reported `DescribeNodeSummary` is the stored node read
/// back — the `Name`, `Role` and `NodeInterfaceMappings` the create stored verbatim,
/// the `Arn` and minted `Id`, the `ClusterId` the path named, the derived
/// `ChannelPlacementGroups` and the `State` the clock owns — and the listing reports
/// it from the same projection `DescribeNode` answers from.
///
/// The rows differ along WHICH optional members the create supplies (spanning the
/// `NodeRole` and `NetworkInterfaceMode` enums and the mapping counts 0, 1 and 2)
/// and along whether a delete has been accepted for the node, because that is what
/// the rule generalises over. Each row is listed before the transition the clock
/// owes has come due and after, since the rule covers the transient state too.
#[tokio::test]
async fn every_listed_node_is_the_stored_node_describe_reports() {
    let table = [
        Stored {
            case: "nothing at all",
            body: || json!({}),
            delete_it: false,
            state: "CREATED",
            settled_state: "CREATED",
        },
        Stored {
            case: "a name only",
            body: || json!({ "name": "studio-a" }),
            delete_it: false,
            state: "CREATED",
            settled_state: "CREATED",
        },
        Stored {
            case: "a name and the ACTIVE role",
            body: || json!({ "name": "studio-b", "role": ROLES[0] }),
            delete_it: false,
            state: "CREATED",
            settled_state: "CREATED",
        },
        Stored {
            case: "the BACKUP role and one interface mapping",
            body: || {
                json!({
                    "name": "studio-c",
                    "role": ROLES[1],
                    "nodeInterfaceMappings": [mapping("my-inputs", "eth1", INTERFACE_MODES[0])],
                })
            },
            delete_it: false,
            state: "CREATED",
            settled_state: "CREATED",
        },
        Stored {
            case: "two interface mappings and tags",
            body: || {
                json!({
                    "name": "studio-d",
                    "nodeInterfaceMappings": [
                        mapping("my-inputs", "eth1", INTERFACE_MODES[0]),
                        mapping("my-outputs", "ENO1234EXAMPLE", INTERFACE_MODES[1]),
                    ],
                    "tags": { "team": "news" },
                })
            },
            delete_it: false,
            state: "CREATED",
            settled_state: "CREATED",
        },
        // A node whose delete has been accepted stays in the listing: the record
        // stays in the account, deregistering and then terminally deregistered.
        Stored {
            case: "a node whose delete was accepted",
            body: || json!({ "name": "on-its-way-out", "role": ROLES[0] }),
            delete_it: true,
            state: "DEREGISTERING",
            settled_state: "DEREGISTERED",
        },
    ];

    for Stored {
        case,
        body,
        delete_it,
        state,
        settled_state,
    } in table
    {
        let router = medialive();
        let listed_cluster = cluster(&router, "listed").await;
        let elsewhere = cluster(&router, "elsewhere").await;

        let node = created(&router, case, &listed_cluster, body()).await;
        let id = node_id(case, &node);
        // One node in ANOTHER cluster, which this listing must not report: a node
        // is named by both its cluster and its own id.
        let hidden = created(&router, case, &elsewhere, json!({ "name": "hidden" })).await;
        let hidden_id = node_id(case, &hidden);

        if delete_it {
            let (status, accepted) = delete_node(&router, &listed_cluster, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode: expected 202, got {status}: {accepted}"
            );
        }

        let (status, page_body) = list(&router, &listed_cluster, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListNodes to answer 200, got {status}: {page_body}"
        );
        let mut page = summaries(case, &page_body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one node of this cluster, got {page_body}"
        );
        assert!(
            page_body["nextToken"].is_null(),
            "case {case:?}: expected no nextToken on the only page, got {page_body}"
        );
        let summary = page.remove(0);
        assert_ne!(
            node_id(case, &summary),
            hidden_id,
            "case {case:?}: expected another cluster's node to stay out of this listing"
        );

        // The listed summary is the created node, member for member — the state
        // aside, since the delete may have moved it.
        assert_eq!(
            without_state(&summary),
            without_state(&node),
            "case {case:?}: expected the listed summary to be the created node, got {summary}"
        );
        assert_eq!(
            summary["state"],
            json!(state),
            "case {case:?}: expected state {state}, got {:?}",
            summary["state"]
        );

        // ... and it is byte for byte what the describe of that node answers.
        let read = described(&router, case, &listed_cluster, &id).await;
        assert_eq!(
            summary, read,
            "case {case:?}: expected the listed summary to equal what DescribeNode reports, \
             got {read}"
        );

        wait_for_the_clock();

        // Once the transition the clock owed has come due the node reports the
        // settled state, and the listing and the describe still agree: there is one
        // stored record, not one per operation.
        let (status, page_body) = list(&router, &listed_cluster, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListNodes to answer 200, got {status}: {page_body}"
        );
        let settled = summaries(case, &page_body).remove(0);
        assert_eq!(
            settled["state"],
            json!(settled_state),
            "case {case:?}: expected the node to settle into {settled_state}, got {:?}",
            settled["state"]
        );
        let read = described(&router, case, &listed_cluster, &id).await;
        assert_eq!(
            settled, read,
            "case {case:?}: expected the listed summary to equal what DescribeNode reports \
             once the clock settled"
        );
        // Every other member survived the transition unchanged.
        assert_eq!(
            without_state(&settled),
            without_state(&summary),
            "case {case:?}: expected only the state to have moved, got {settled}"
        );
    }
}

/// Rule 3: a listing request the service cannot read is refused with the modelled
/// 400 `BadRequestException` and changes nothing, a `NextToken` naming a position
/// past the end of the collection reports an empty last page rather than failing a
/// walk that was valid when it began, and the listing is a READ — walking it twice
/// reports the same nodes.
///
/// The rows differ along HOW the request is out of bounds (a page size of zero, a
/// negative one, a large negative one; a cluster id that names nothing, one that
/// names another resource type and one that is not an id at all) and along the FORM
/// of the resume token (an id past every minted one, a non-numeric string, a token
/// below every minted id and another resource type's id), because that is what the
/// rule generalises over.
#[tokio::test]
async fn a_node_listing_request_the_service_cannot_satisfy_is_refused() {
    let router = medialive();
    let cluster_id = cluster(&router, "listed").await;

    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        let node = created(
            &router,
            "the bystanders",
            &cluster_id,
            json!({ "name": format!("bystander-{index}") }),
        )
        .await;
        minted.push(node_id("the bystanders", &node));
    }

    // A page size that asks for nothing at all.
    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &cluster_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {max_results}: expected a non-positive maxResults to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {max_results}: expected BadRequestException, got {body}"
        );
    }

    // A `ClusterId` that names no cluster names no collection: this operation
    // models no 404, so it is the request that cannot be read.
    for (case, bad_cluster) in [
        ("no such cluster", "9999999".to_string()),
        ("not an id at all", "no-such-cluster".to_string()),
        ("another resource type's id", minted[0].clone()),
    ] {
        let (status, body) = list(&router, &bad_cluster, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a cluster-less path to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // A resume token no page would hand out. `NextToken` is the id the next page
    // starts at, so a token past every minted id resumes at the end of the
    // collection: an empty page with no further token.
    for (case, token) in [
        ("past every minted id", "99999999"),
        ("not an id at all", "zzz-no-such-node"),
    ] {
        let (status, body) = list(&router, &cluster_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a token past the end to report an empty page, \
             got {status}: {body}"
        );
        assert!(
            summaries(case, &body).is_empty(),
            "case {case:?}: expected no summaries past the end, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: expected no further token past the end, got {body}"
        );
    }

    // A token BELOW every minted id resumes at the first one: the scan is a range
    // over the ids, not a positional offset into a page the caller must have seen.
    // Each resource type's ids come from a range of its own, and a cluster's are
    // below a node's, so the cluster's own id is one such token.
    for (case, token) in [
        ("zero", "0".to_string()),
        ("a cluster's id", cluster_id.clone()),
    ] {
        let (status, body) = list(&router, &cluster_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            summaries(case, &body)
                .iter()
                .map(|summary| node_id(case, summary))
                .collect::<Vec<_>>(),
            minted,
            "case {case:?}: expected the walk to start at the first node"
        );
    }

    // The refusals stored nothing, and the listing itself changed nothing: two
    // consecutive walks report the same nodes.
    let (first, pages) = every_page(&router, "the first walk", &cluster_id, None).await;
    let (second, _) = every_page(&router, "the second walk", &cluster_id, None).await;
    assert_eq!(
        first, minted,
        "expected the refused requests to have stored nothing; got {first:?} over {pages:?}"
    );
    assert_eq!(
        second, first,
        "expected a read not to change what a later caller sees"
    );
}
