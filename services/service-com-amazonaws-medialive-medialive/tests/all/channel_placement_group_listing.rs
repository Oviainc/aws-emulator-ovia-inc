// SPDX-License-Identifier: CC0-1.0

//! `ListChannelPlacementGroups` — the channel placement group collection of ONE
//! MediaLive Anywhere cluster — end to end over the restJson1 wire.
//!
//! The rules the suites below assert, each over a TABLE that spans its class
//! rather than one recorded request:
//!
//! 1. The listing reports the groups the store holds for the `ClusterId` the path
//!    names, each exactly once, in the order their ids were minted, cut into pages
//!    of at most `MaxResults` (bounded by the service's own ceiling,
//!    `MAX_LIST_PAGE_SIZE`) whose `NextToken` resumes at the first group not yet
//!    reported and is absent on the last page.
//! 2. Every member of a reported `DescribeChannelPlacementGroupSummary` is the
//!    stored group read back — the same projection
//!    `DescribeChannelPlacementGroup` answers from — so a listed summary and a
//!    described group can never disagree, in the transient `ASSIGNING` state or
//!    once the clock has made `ASSIGNED` due; and a group is reported only through
//!    the cluster it was created in.
//! 3. A listing request the service cannot read (a `MaxResults` below 1, or a
//!    `ClusterId` naming no cluster) is refused with the modelled 400
//!    `BadRequestException` and changes nothing, a `NextToken` past the end of the
//!    collection reports an empty last page, and the listing itself is a READ: two
//!    consecutive walks report the same groups.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::channel_placement_groups::{
    create_channel, create_cluster, create_nodes, created, described, groups_uri, id_of,
    without_state,
};
use crate::wire::{call, error_code, medialive};

/// `ListChannelPlacementGroups` on `cluster`, with `query` appended verbatim
/// (`MaxResults` and `NextToken` are both `@httpQuery`).
async fn list(router: &MultiServiceRouter, cluster: &str, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{}{query}", groups_uri(cluster)), None).await
}

/// The `channelPlacementGroups` member as a list, whatever its length. An empty
/// list and an absent member both mean "no entries" to a client, and which of the
/// two the restJson1 codec writes is the serializer's choice, so both read here as
/// the empty list.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    match body.get("channelPlacementGroups") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a channelPlacementGroups list, got {other}"),
    }
}

/// One group description with the two DERIVED members dropped — `State`, which
/// the clock moves, and `Channels`, which follows the channel store — so a
/// listing and the create that stored the group can be compared on every member
/// the create itself supplied.
fn stored_members(group: &Value) -> Value {
    let mut copy = without_state(group);
    if let Some(object) = copy.as_object_mut() {
        object.remove("channels");
    }
    copy
}

/// Walk every page of one cluster's listing, answering the ids reported in order
/// and the size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
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

        let (status, body) = list(router, cluster, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListChannelPlacementGroups to answer 200, \
             got {status}: {body}"
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
            ids.push(id_of(summary));
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

/// One row of the pagination table: how many groups the cluster holds, how many a
/// SECOND cluster holds beside it (so the walk is asserted to be scoped), and the
/// page size the request asks for.
struct Paging {
    case: &'static str,
    groups: usize,
    bystanders: usize,
    page_size: Option<i32>,
}

/// Rule 1: the listing reports the groups the store holds for the cluster the path
/// names, each exactly once, in the order their ids were minted, cut into pages of
/// at most `MaxResults` (bounded by the service's own ceiling) whose `NextToken`
/// resumes at the first group not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of groups in the listed cluster, the count in a
/// second cluster whose groups interleave with them in the one id space, and the
/// requested page size — including none stored at all (the stub issue's own
/// reproduction: a listing on a cluster no create has placed a group in), a page
/// size of one, a size that divides the collection unevenly, no size at all, a size
/// one past the collection and a size past the service's ceiling — because that is
/// what the rule generalises over.
#[tokio::test]
async fn the_listing_pages_through_every_group_of_the_cluster_in_mint_order() {
    let table = [
        Paging {
            case: "no groups at all",
            groups: 0,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "no groups, another cluster has two",
            groups: 0,
            bystanders: 2,
            page_size: Some(1),
        },
        Paging {
            case: "one group, page of one",
            groups: 1,
            bystanders: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three groups, page of one",
            groups: 3,
            bystanders: 2,
            page_size: Some(1),
        },
        Paging {
            case: "five groups, page of two",
            groups: 5,
            bystanders: 3,
            page_size: Some(2),
        },
        Paging {
            case: "four groups, default page size",
            groups: 4,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "three groups, page of four",
            groups: 3,
            bystanders: 1,
            page_size: Some(4),
        },
        Paging {
            case: "three groups, page size at the ceiling",
            groups: 3,
            bystanders: 0,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32),
        },
        Paging {
            case: "two groups, page size past the ceiling",
            groups: 2,
            bystanders: 2,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        // More groups than one page can carry whatever the request asks for, so
        // the ceiling itself is what cuts the pages.
        Paging {
            case: "past the ceiling, no page size",
            groups: MAX_LIST_PAGE_SIZE + 3,
            bystanders: 0,
            page_size: None,
        },
        Paging {
            case: "past the ceiling, page size past the ceiling",
            groups: MAX_LIST_PAGE_SIZE + 3,
            bystanders: 2,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 * 2),
        },
    ];

    for Paging {
        case,
        groups,
        bystanders,
        page_size,
    } in table
    {
        let router = medialive();
        let listed_cluster = create_cluster(&router, case, "listed").await;
        let other_cluster = create_cluster(&router, case, "other").await;

        // The two clusters' groups are created ALTERNATELY, so their ids
        // interleave in the one minted id space and a listing that ignored the
        // cluster scope could not answer the expected ids.
        let mut minted: Vec<String> = Vec::new();
        for index in 0..groups.max(bystanders) {
            if index < groups {
                let group = created(
                    &router,
                    case,
                    &listed_cluster,
                    json!({ "name": format!("listed-{index}") }),
                )
                .await;
                minted.push(id_of(&group));
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
            "case {case:?}: expected the listing to report every group of the cluster once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n` every page but
        // the last carries exactly `n`.
        let effective = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if groups == 0 {
            1
        } else {
            groups.div_ceil(effective)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {groups} groups of \
             {effective}, got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                groups - effective * index
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

/// One row of the stored-group table: what the create request carries (built
/// from the node ids the row's cluster really has, since `Nodes` is a reference
/// to a node of that cluster) and whether a channel is placed in the group it
/// creates.
struct Stored {
    case: &'static str,
    body: fn(&[String]) -> Value,
    place_a_channel: bool,
    /// The `ChannelPlacementGroupState` the group reports straight after the
    /// create and the one the clock owes it, from the model's own enum
    /// (`smithy_query(query="shape", shape="ChannelPlacementGroupState")`).
    state: &'static str,
    settled_state: &'static str,
}

/// Rule 2: every member of a reported `DescribeChannelPlacementGroupSummary` is the
/// stored group read back — the `Name` and `Nodes` the create stored verbatim, the
/// `Arn` built from the signed region and the minted `Id` that is its resource-id
/// portion, the `ClusterId` the path named, the `Channels` derived from the channels
/// placed in the group and the `State` the clock owns — and the listing reports it
/// from the same projection `DescribeChannelPlacementGroup` answers from.
///
/// The rows differ along WHICH optional members the create supplies, along whether
/// a channel is placed in the group (the derived member) and so along which state
/// the group is in, because that is what the rule generalises over. Each row is
/// listed before the node association has come due and after, since the rule covers
/// the transient state too.
#[tokio::test]
async fn every_listed_group_is_the_stored_group_describe_reports() {
    let table = [
        Stored {
            case: "nothing at all",
            body: |_| json!({}),
            place_a_channel: false,
            state: "UNASSIGNED",
            settled_state: "UNASSIGNED",
        },
        Stored {
            case: "a name only",
            body: |_| json!({ "name": "studio-a" }),
            place_a_channel: false,
            state: "UNASSIGNED",
            settled_state: "UNASSIGNED",
        },
        Stored {
            case: "a name and one node",
            body: |nodes| json!({ "name": "studio-b", "nodes": [nodes[0].clone()] }),
            place_a_channel: false,
            state: "ASSIGNING",
            settled_state: "ASSIGNED",
        },
        Stored {
            case: "tags too, and a channel placed in it",
            body: |_| json!({ "name": "tagged", "tags": { "team": "news" } }),
            place_a_channel: true,
            state: "UNASSIGNED",
            settled_state: "UNASSIGNED",
        },
        Stored {
            case: "an empty node list, and a channel placed in it",
            body: |_| json!({ "name": "no-nodes", "nodes": [] }),
            place_a_channel: true,
            state: "UNASSIGNED",
            settled_state: "UNASSIGNED",
        },
    ];

    for Stored {
        case,
        body,
        place_a_channel,
        state,
        settled_state,
    } in table
    {
        let router = medialive();
        let cluster = create_cluster(&router, case, "listed").await;
        let elsewhere = create_cluster(&router, case, "elsewhere").await;
        // The node a row may associate its group with: a node of the group's own
        // cluster is the only kind `Nodes` may name.
        let nodes = create_nodes(&router, case, &cluster, 1).await;

        let group = created(&router, case, &cluster, body(&nodes)).await;
        let id = id_of(&group);
        // One group in ANOTHER cluster, which this listing must not report: a
        // group is named by both path labels.
        let hidden = created(&router, case, &elsewhere, json!({ "name": "hidden" })).await;
        let hidden_id = id_of(&hidden);

        let channel = if place_a_channel {
            Some(create_channel(&router, case, "placed", Some((&cluster, &id))).await)
        } else {
            None
        };

        let (status, body) = list(&router, &cluster, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListChannelPlacementGroups to answer 200, \
             got {status}: {body}"
        );
        let mut page = summaries(case, &body);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one group of this cluster, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: expected no nextToken on the only page, got {body}"
        );
        let summary = page.remove(0);
        assert_ne!(
            id_of(&summary),
            hidden_id,
            "case {case:?}: expected another cluster's group to stay out of this listing"
        );

        // The listed summary is the created group, member for member — the two
        // derived members aside, since a channel placed after the create changes
        // `Channels` and the clock may move `State`.
        assert_eq!(
            stored_members(&summary),
            stored_members(&group),
            "case {case:?}: expected the listed summary to be the created group, got {summary}"
        );
        assert_eq!(
            summary["state"],
            json!(state),
            "case {case:?}: expected state {state}, got {:?}",
            summary["state"]
        );
        // `Channels` is derived from the channel store on every read, so the
        // channel placed after the create shows up in the listing.
        assert_eq!(
            summary["channels"],
            json!(channel.as_ref().map(|id| vec![id.clone()]).unwrap_or_default()),
            "case {case:?}: expected the placed channels, got {:?}",
            summary["channels"]
        );

        // ... and it is byte for byte what the describe of that group answers.
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            without_state(&summary),
            without_state(&read),
            "case {case:?}: expected the listed summary to equal what \
             DescribeChannelPlacementGroup reports, got {read}"
        );

        wait_for_the_clock();

        // Once the association has come due the group reports the settled state,
        // and the listing and the describe still agree: there is one stored
        // record, not one per operation.
        let (status, body) = list(&router, &cluster, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListChannelPlacementGroups to answer 200, \
             got {status}: {body}"
        );
        let settled = summaries(case, &body).remove(0);
        assert_eq!(
            settled["state"],
            json!(settled_state),
            "case {case:?}: expected the group to settle into {settled_state}, got {:?}",
            settled["state"]
        );
        let read = described(&router, case, &cluster, &id).await;
        assert_eq!(
            settled, read,
            "case {case:?}: expected the listed summary to equal what \
             DescribeChannelPlacementGroup reports once the association settled"
        );
        // Every other member survived the transition unchanged.
        assert_eq!(
            without_state(&settled),
            without_state(&summary),
            "case {case:?}: expected only the state to have moved, got {settled}"
        );
        assert_eq!(
            stored_members(&settled),
            stored_members(&group),
            "case {case:?}: expected the create's own members to be unchanged, got {settled}"
        );
    }
}

/// Rule 3: a listing request the service cannot read is refused with the modelled
/// 400 `BadRequestException` and changes nothing, a `NextToken` naming a position
/// past the end of the collection reports an empty last page rather than failing a
/// walk that was valid when it began, and the listing is a READ — walking it twice
/// reports the same groups.
///
/// The rows differ along HOW the request is out of bounds (a page size of zero, a
/// negative one, a large negative one; a cluster id that names nothing, one that
/// names another resource type and one that is not an id at all) and along the FORM
/// of the resume token (an id past every minted one, a non-numeric string, a token
/// below every minted id and another resource type's id), because that is what the
/// rule generalises over.
#[tokio::test]
async fn a_listing_request_the_service_cannot_satisfy_is_refused() {
    let router = medialive();
    let cluster = create_cluster(&router, "the bystanders", "listed").await;

    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        let group = created(
            &router,
            "the bystanders",
            &cluster,
            json!({ "name": format!("bystander-{index}") }),
        )
        .await;
        minted.push(id_of(&group));
    }

    // A page size that asks for nothing at all.
    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &cluster, &format!("?maxResults={max_results}")).await;
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
        ("not an id at all", "zzz-no-such-group"),
    ] {
        let (status, body) = list(&router, &cluster, &format!("?nextToken={token}")).await;
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
    // below a placement group's, so the cluster's own id is one such token.
    for (case, token) in [("zero", "0".to_string()), ("a cluster's id", cluster.clone())] {
        let (status, body) = list(&router, &cluster, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            summaries(case, &body)
                .iter()
                .map(id_of)
                .collect::<Vec<_>>(),
            minted,
            "case {case:?}: expected the walk to start at the first group"
        );
    }

    // The refusals stored nothing, and the listing itself changed nothing: two
    // consecutive walks report the same groups.
    let (first, pages) = every_page(&router, "the first walk", &cluster, None).await;
    let (second, _) = every_page(&router, "the second walk", &cluster, None).await;
    assert_eq!(
        first, minted,
        "expected the refused requests to have stored nothing; got {first:?} over {pages:?}"
    );
    assert_eq!(
        second, first,
        "expected a read not to change what a later caller sees"
    );
}
