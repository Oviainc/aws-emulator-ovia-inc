// SPDX-License-Identifier: CC0-1.0

//! `ListClusters` — MediaLive Anywhere's cluster collection listing — end to end
//! over the restJson1 wire.
//!
//! The rules the suites below assert, each over a TABLE that spans its class
//! rather than one recorded request:
//!
//! 1. `ListClusters` reports the clusters this service's store holds, each
//!    exactly once, in the order their ids were minted, cut into pages of at most
//!    `MaxResults` (bounded by the service's own ceiling,
//!    `MAX_LIST_PAGE_SIZE`) whose `NextToken` resumes at the first cluster not yet
//!    reported and is absent on the last page.
//! 2. Every member of a reported `DescribeClusterSummary` is the stored cluster
//!    read back — the same projection `DescribeCluster` answers from — so a listed
//!    summary and a described cluster can never disagree, in the transient
//!    `CREATING` state or once the clock has made `ACTIVE` due.
//! 3. A listing request the service cannot read (a `MaxResults` below 1) is
//!    refused with the modelled 400 `BadRequestException` and changes nothing,
//!    and the listing itself is a READ: two consecutive walks report the same
//!    clusters.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::member;
use crate::wire::{call, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The cluster collection URI `CreateCluster` and `ListClusters` are bound to
/// (`smithy_query(query="operation",operation="CreateCluster,ListClusters")`:
/// POST / GET /prod/clusters).
const CLUSTERS: &str = "/prod/clusters";

/// The only `ClusterType` the model declares
/// (`smithy_query(query="shape",shape="ClusterType")`: ON_PREMISES).
const ON_PREMISES: &str = "ON_PREMISES";

/// `CreateCluster` with `body`, answering the created cluster, with the create
/// itself asserted to have answered the modelled 201.
async fn created(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, cluster) = call(router, "POST", CLUSTERS, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateCluster to answer 201, got {status}: {cluster}"
    );
    cluster
}

/// `DescribeCluster` on `id`.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{CLUSTERS}/{id}"), None).await
}

/// `ListClusters` with `query` appended verbatim (both input members are
/// `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{CLUSTERS}{query}"), None).await
}

/// The `clusters` member as a list, whatever its length. An empty list and an
/// absent member both mean "no entries" to a client, and which of the two the
/// restJson1 codec writes is the serializer's choice, so both read here as the
/// empty list.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    match body.get("clusters") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(other) => panic!("case {case:?}: expected a clusters list, got {other}"),
    }
}

/// The `id` member of a response, as a client reads it.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an id, got {body}"))
        .to_string()
}

/// Walk every page of the listing, answering the ids reported in order and the
/// size of each page.
async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
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

        let (status, body) = list(router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListClusters to answer 200, got {status}: {body}"
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
            ids.push(id_of(case, summary));
        }

        token = body["nextToken"].as_str().map(ToString::to_string);
        // A token is handed out only when something is left over, so an empty
        // page can never carry one.
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

/// One row of the pagination table: how many clusters the store holds and the
/// page size the request asks for.
struct Paging {
    case: &'static str,
    clusters: usize,
    page_size: Option<i32>,
}

/// Rule 1: `ListClusters` reports the clusters this service's store holds, each
/// exactly once, in the order their ids were minted, cut into pages of at most
/// `MaxResults` (bounded by the service's own ceiling) whose `NextToken` resumes
/// at the first cluster not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of stored clusters and the requested page
/// size — including none stored (the stub issue's own reproduction: a
/// `ListClusters` call on a store no create has written to), a page size of one,
/// a size that divides the store unevenly, no size at all, a size one past the
/// store's content and a size past the service's ceiling — because that is what
/// the rule generalises over.
#[tokio::test]
async fn list_clusters_pages_through_every_stored_cluster_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            clusters: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            clusters: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one cluster, page of one",
            clusters: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three clusters, page of one",
            clusters: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five clusters, page of two",
            clusters: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four clusters, default page size",
            clusters: 4,
            page_size: None,
        },
        Paging {
            case: "three clusters, page of four",
            clusters: 3,
            page_size: Some(4),
        },
        Paging {
            case: "three clusters, page size at the ceiling",
            clusters: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32),
        },
        Paging {
            case: "two clusters, page size past the ceiling",
            clusters: 2,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
        // More clusters than one page can carry whatever the request asks for,
        // so the ceiling itself is what cuts the pages.
        Paging {
            case: "past the ceiling, no page size",
            clusters: MAX_LIST_PAGE_SIZE + 3,
            page_size: None,
        },
        Paging {
            case: "past the ceiling, page size past the ceiling",
            clusters: MAX_LIST_PAGE_SIZE + 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 * 2),
        },
    ];

    for Paging {
        case,
        clusters,
        page_size,
    } in table
    {
        let router = medialive();

        let mut minted: Vec<String> = Vec::new();
        for index in 0..clusters {
            let cluster = created(
                &router,
                case,
                json!({
                    "name": format!("cluster-{index}"),
                    "clusterType": ON_PREMISES,
                }),
            )
            .await;
            minted.push(id_of(case, &cluster));
        }

        let (listed, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            listed, minted,
            "case {case:?}: expected the listing to report every stored cluster once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n` every page but
        // the last carries exactly `n`.
        let effective = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if clusters == 0 {
            1
        } else {
            clusters.div_ceil(effective)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {clusters} clusters \
             of {effective}, got {pages:?}"
        );
        for (index, page) in pages.iter().enumerate() {
            let expected = if index + 1 == pages.len() {
                clusters - effective * index
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

/// One row of the stored-cluster table: what the create request carries.
struct Stored {
    case: &'static str,
    body: fn() -> Value,
}

/// Rule 2: every member of a reported `DescribeClusterSummary` is the stored
/// cluster read back — the `Name`, `ClusterType`, `InstanceRoleArn` and
/// `NetworkSettings` the create stored verbatim, the `Arn` built from the
/// request's region and the minted `Id` that is its resource-id portion, the
/// `ChannelIds` derived from the channels attached to the cluster and the `State`
/// the clock owns — and `ListClusters` reports it from the same projection
/// `DescribeCluster` answers from.
///
/// The rows differ along WHICH optional members the create supplies and along the
/// SIZE of the interface-mapping list (none at all, no network settings, one
/// mapping, several, no name), because that is what the rule generalises over.
/// Each row is listed before the creation has come due and after, since the rule
/// covers the transient state too.
#[tokio::test]
async fn every_listed_cluster_is_the_stored_cluster_describe_reports() {
    let table = [
        Stored {
            case: "nothing but a type",
            body: || json!({ "name": "bare", "clusterType": ON_PREMISES }),
        },
        Stored {
            case: "no name at all",
            body: || json!({ "clusterType": ON_PREMISES }),
        },
        Stored {
            case: "a role and one interface mapping",
            body: || {
                json!({
                    "name": "one-mapping",
                    "clusterType": ON_PREMISES,
                    "instanceRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MediaLiveAnywhere"),
                    "networkSettings": {
                        "defaultRoute": "net-1",
                        "interfaceMappings": [
                            { "logicalInterfaceName": "primary", "networkId": "net-1" }
                        ]
                    },
                })
            },
        },
        Stored {
            case: "three interface mappings and tags",
            body: || {
                json!({
                    "name": "three-mappings",
                    "clusterType": ON_PREMISES,
                    "networkSettings": {
                        "defaultRoute": "net-a",
                        "interfaceMappings": [
                            { "logicalInterfaceName": "primary", "networkId": "net-a" },
                            { "logicalInterfaceName": "backup", "networkId": "net-b" },
                            { "logicalInterfaceName": "management", "networkId": "net-c" }
                        ]
                    },
                    "tags": { "team": "video" },
                })
            },
        },
        Stored {
            case: "empty interface mappings",
            body: || {
                json!({
                    "name": "no-mappings",
                    "clusterType": ON_PREMISES,
                    "networkSettings": { "interfaceMappings": [] },
                })
            },
        },
    ];

    for Stored { case, body } in table {
        let router = medialive();
        let body = body();
        let cluster = created(&router, case, body.clone()).await;
        let id = id_of(case, &cluster);

        // `Id` is the resource-id portion of the `Arn`, which names the region
        // the request was signed for and the emulator's account.
        assert_eq!(
            cluster["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:cluster:{id}"
            )),
            "case {case:?}: expected the ARN to carry the region, the account and the id, \
             got {:?}",
            cluster["arn"]
        );
        // The `ClusterState` a created cluster starts in: creation is
        // asynchronous, so the caller that lists straight after the create still
        // sees the transient state.
        assert_eq!(
            cluster["state"],
            json!("CREATING"),
            "case {case:?}: expected a freshly created cluster to be CREATING, got {:?}",
            cluster["state"]
        );

        // The listing reports the one stored cluster, member for member as the
        // create supplied it, and byte for byte as the describe answers.
        let (status, listed) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListClusters to answer 200, got {status}: {listed}"
        );
        let mut page = summaries(case, &listed);
        assert_eq!(
            page.len(),
            1,
            "case {case:?}: expected the one stored cluster, got {listed}"
        );
        let summary = page.remove(0);
        assert!(
            listed["nextToken"].is_null(),
            "case {case:?}: expected no nextToken on the only page, got {listed}"
        );

        for stored in ["name", "clusterType", "instanceRoleArn", "networkSettings"] {
            assert_eq!(
                member(&summary, stored),
                member(&body, stored),
                "case {case:?}: expected the listed {stored} to be the one the request \
                 supplied, got {:?}",
                member(&summary, stored)
            );
        }
        // Nothing has created a channel in this cluster, so `ChannelIds` is an
        // empty list rather than an unknown one.
        assert_eq!(
            summary["channelIds"],
            json!([]),
            "case {case:?}: expected no associated channels, got {:?}",
            summary["channelIds"]
        );

        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCluster to answer 200, got {status}: {described}"
        );
        assert_eq!(
            summary, described,
            "case {case:?}: expected the listed summary to equal what DescribeCluster reports"
        );

        wait_for_the_clock();

        // Once the creation has come due the cluster is ACTIVE, and the listing
        // and the describe still agree: there is one stored record, not one per
        // operation.
        let (status, settled) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListClusters to answer 200, got {status}: {settled}"
        );
        let settled = summaries(case, &settled).remove(0);
        assert_eq!(
            settled["state"],
            json!("ACTIVE"),
            "case {case:?}: expected the creation to have settled, got {:?}",
            settled["state"]
        );
        let (_, described) = describe(&router, &id).await;
        assert_eq!(
            settled, described,
            "case {case:?}: expected the listed summary to equal what DescribeCluster reports \
             once the creation settled"
        );
        // Every other member survived the transition unchanged.
        for stored in [
            "arn",
            "id",
            "name",
            "clusterType",
            "instanceRoleArn",
            "networkSettings",
            "channelIds",
        ] {
            assert_eq!(
                member(&settled, stored),
                member(&summary, stored),
                "case {case:?}: expected {stored} to be unchanged by the transition, got {:?}",
                member(&settled, stored)
            );
        }
    }
}

/// Rule 3: a listing request the service cannot read is refused with the modelled
/// 400 `BadRequestException` and changes nothing, a `NextToken` naming a position
/// past the end of the collection reports an empty last page rather than failing a
/// walk that was valid when it began, and the listing is a READ — walking it twice
/// reports the same clusters.
///
/// The rows differ along HOW the request is out of bounds (zero, one negative, a
/// large negative) and along the FORM of the resume token (an id past every
/// minted one, a non-numeric string, a token below every minted id and another
/// resource type's id), because that is what the rule generalises over.
#[tokio::test]
async fn a_listing_request_the_service_cannot_satisfy_is_refused() {
    let router = medialive();

    let mut minted: Vec<String> = Vec::new();
    for index in 0..3 {
        let cluster = created(
            &router,
            "the bystanders",
            json!({
                "name": format!("bystander-{index}"),
                "clusterType": ON_PREMISES,
            }),
        )
        .await;
        minted.push(id_of("the bystanders", &cluster));
    }

    // A page size that asks for nothing at all.
    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &format!("?maxResults={max_results}")).await;
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

    // A resume token no page would hand out. `NextToken` is the id the next page
    // starts at, so a token past every minted id resumes at the end of the
    // collection: an empty page with no further token.
    for (case, token) in [
        ("past every minted id", "99999999"),
        ("not an id at all", "zzz-no-such-cluster"),
    ] {
        let (status, body) = list(&router, &format!("?nextToken={token}")).await;
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
    // Each resource type's ids come from a range of its own, and a channel's are
    // below a cluster's, so another type's id is one such token.
    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "neighbour" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {channel}"
    );
    let channel_id = id_of("a channel's id", &channel["channel"]);
    for (case, token) in [
        ("zero", "0".to_string()),
        ("a channel's id", channel_id.clone()),
    ] {
        let (status, body) = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            summaries(case, &body)
                .iter()
                .map(|summary| id_of(case, summary))
                .collect::<Vec<_>>(),
            minted,
            "case {case:?}: expected the walk to start at the first cluster"
        );
    }

    // The refusals stored nothing, and the listing itself changed nothing: two
    // consecutive walks report the same clusters.
    let (first, pages) = every_page(&router, "the first walk", None).await;
    let (second, _) = every_page(&router, "the second walk", None).await;
    assert_eq!(
        first, minted,
        "expected the refused requests to have stored nothing; got {first:?} over {pages:?}"
    );
    assert_eq!(
        second, first,
        "expected a read not to change what a later caller sees"
    );
}
