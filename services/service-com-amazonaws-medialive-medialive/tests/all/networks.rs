// SPDX-License-Identifier: CC0-1.0

//! The MediaLive Anywhere network resource — `CreateNetwork`, `DescribeNetwork`
//! and `ListNetworks` — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: a single `ListNetworks` call on a store nothing has
//! created a network in (the issue's own reproduction) is ONE row of the first of
//! them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    MAX_LIST_PAGE_SIZE, MAX_NETWORK_CIDR_ENTRIES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URI `CreateNetwork` and `ListNetworks` are bound to
/// (`smithy_query(query="operation", operation="CreateNetwork,ListNetworks")`:
/// POST / GET /prod/networks).
const NETWORKS: &str = "/prod/networks";

/// `CreateNetwork` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", NETWORKS, Some(body)).await
}

/// `CreateNetwork` with `body`, answering the created network, with the create
/// itself asserted to have answered the modelled 201.
async fn created(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, network) = create(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateNetwork to answer 201, got {status}: {network}"
    );
    network
}

/// `DescribeNetwork` on `id`.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{NETWORKS}/{id}"), None).await
}

/// `ListNetworks` with `query` appended verbatim (both input members are
/// `@httpQuery`).
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("{NETWORKS}{query}"), None).await
}

/// The `networks` member as a list, whatever its length.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["networks"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a networks list, got {body}"))
}

/// A list member as a client reads it: an empty list and an absent member both
/// mean "no entries", and which of the two the restJson1 codec writes for an
/// empty list is the serializer's choice, so both read here as the empty list.
fn list_member(body: &Value, name: &str) -> Value {
    match body.get(name) {
        None | Some(Value::Null) => json!([]),
        Some(other) => other.clone(),
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
            "case {case:?}: expected ListNetworks to answer 200, got {status}: {body}"
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
            pages.len() <= 64,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (ids, pages)
}

/// One row of the pagination table: how many networks the store holds and the
/// page size the request asks for.
struct Paging {
    case: &'static str,
    networks: usize,
    page_size: Option<i32>,
}

/// Rule: `ListNetworks` reports the networks this service's store holds, each
/// exactly once, in the order their ids were minted, cut into pages of at most
/// `MaxResults` (bounded by the service's own ceiling) whose `NextToken` resumes
/// at the first network not yet reported and is absent on the last page.
///
/// The rows differ along the COUNT of stored networks and the requested page
/// size — including none stored (the issue's own reproduction: a `ListNetworks`
/// call on a store no create has written to), a page size of one, a size that
/// divides the store unevenly, no size at all, and a size past the service's
/// ceiling — because that is what the rule generalises over.
#[tokio::test]
async fn list_networks_pages_through_every_stored_network_in_mint_order() {
    let table = [
        Paging {
            case: "empty store",
            networks: 0,
            page_size: None,
        },
        Paging {
            case: "empty store, page of one",
            networks: 0,
            page_size: Some(1),
        },
        Paging {
            case: "one network, page of one",
            networks: 1,
            page_size: Some(1),
        },
        Paging {
            case: "three networks, page of one",
            networks: 3,
            page_size: Some(1),
        },
        Paging {
            case: "five networks, page of two",
            networks: 5,
            page_size: Some(2),
        },
        Paging {
            case: "four networks, default page size",
            networks: 4,
            page_size: None,
        },
        Paging {
            case: "three networks, page size past the ceiling",
            networks: 3,
            page_size: Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        },
    ];

    for Paging {
        case,
        networks,
        page_size,
    } in table
    {
        let router = medialive();

        let mut minted: Vec<String> = Vec::new();
        for index in 0..networks {
            let network = created(
                &router,
                case,
                json!({
                    "name": format!("network-{index}"),
                    "ipPools": [{ "cidr": format!("10.{index}.0.0/16") }],
                }),
            )
            .await;
            minted.push(id_of(case, &network));
        }

        let (listed, pages) = every_page(&router, case, page_size).await;
        assert_eq!(
            listed, minted,
            "case {case:?}: expected the listing to report every stored network once, \
             in mint order; pages were {pages:?}"
        );

        // The page cut itself: with a page size of `n` every page but the last
        // carries exactly `n`.
        if let Some(size) = page_size.map(|size| (size as usize).min(MAX_LIST_PAGE_SIZE)) {
            let expected_pages = if networks == 0 {
                1
            } else {
                networks.div_ceil(size)
            };
            assert_eq!(
                pages.len(),
                expected_pages,
                "case {case:?}: expected {expected_pages} pages for {networks} networks \
                 of {size}, got {pages:?}"
            );
            for (index, page) in pages.iter().enumerate() {
                let expected = if index + 1 == pages.len() {
                    networks - size * index
                } else {
                    size
                };
                assert_eq!(
                    *page, expected,
                    "case {case:?}: expected page {index} to carry {expected} summaries, \
                     got {pages:?}"
                );
            }
        }
    }
}

/// One row of the stored-network table: what the create request carries beyond
/// nothing at all.
struct Stored {
    case: &'static str,
    name: Option<&'static str>,
    ip_pools: Value,
    routes: Value,
}

/// Rule: every member a network response carries is the stored network read back
/// — the `Name`, `IpPools` and `Routes` the create stored verbatim, the `Arn`
/// built from the request's region and the minted `Id` that is its resource-id
/// portion, the `AssociatedClusterIds` derived from the clusters attached to it,
/// and the `State` the clock owns — and `CreateNetwork`, `DescribeNetwork` and
/// `ListNetworks` all report it from that one stored record.
///
/// The rows differ along WHICH optional members the create supplies and along the
/// SIZE and CONTENT of the two CIDR lists (absent, empty, one entry, several, a
/// route with a gateway and one without), because that is what the rule
/// generalises over. Each row is read before the creation has come due and after,
/// since the rule covers the transient state too.
#[tokio::test]
async fn every_network_member_is_the_stored_network_read_back() {
    let table = [
        Stored {
            case: "nothing but a name",
            name: Some("bare"),
            ip_pools: Value::Null,
            routes: Value::Null,
        },
        Stored {
            case: "no name at all",
            name: None,
            ip_pools: Value::Null,
            routes: Value::Null,
        },
        Stored {
            case: "empty lists",
            name: Some("empty-lists"),
            ip_pools: json!([]),
            routes: json!([]),
        },
        Stored {
            case: "one pool",
            name: Some("one-pool"),
            ip_pools: json!([{ "cidr": "192.0.2.0/24" }]),
            routes: Value::Null,
        },
        Stored {
            case: "three pools and two routes",
            name: Some("three-pools"),
            ip_pools: json!([
                { "cidr": "192.0.2.0/24" },
                { "cidr": "198.51.100.128/25" },
                { "cidr": "10.0.0.0/8" },
            ]),
            routes: json!([
                { "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" },
                { "cidr": "203.0.113.0/24" },
            ]),
        },
        Stored {
            case: "a route without a gateway only",
            name: Some("gatewayless"),
            ip_pools: json!([{ "cidr": "172.16.0.0/12" }]),
            routes: json!([{ "cidr": "172.16.0.0/12" }]),
        },
        Stored {
            case: "the published prefix bounds",
            name: Some("prefix-bounds"),
            // The bounds of an IPv4 prefix length: /0 and /32.
            ip_pools: json!([{ "cidr": "0.0.0.0/0" }, { "cidr": "192.0.2.7/32" }]),
            routes: Value::Null,
        },
    ];

    for Stored {
        case,
        name,
        ip_pools,
        routes,
    } in table
    {
        let router = medialive();

        let mut body = json!({});
        if let Some(name) = name {
            body["name"] = json!(name);
        }
        if !ip_pools.is_null() {
            body["ipPools"] = ip_pools.clone();
        }
        if !routes.is_null() {
            body["routes"] = routes.clone();
        }
        let network = created(&router, case, body).await;
        let id = id_of(case, &network);

        // `Id` is the resource-id portion of the `Arn`, which names the region
        // the request was signed for and the emulator's account.
        assert_eq!(
            network["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:network:{id}"
            )),
            "case {case:?}: expected the ARN to carry the region, the account and the id, \
             got {:?}",
            network["arn"]
        );

        // The two CIDR lists are what the request supplied, in its order, and a
        // request that named none reports none rather than an invented pool.
        let expected_pools = if ip_pools.is_null() {
            json!([])
        } else {
            ip_pools.clone()
        };
        let expected_routes = if routes.is_null() {
            json!([])
        } else {
            routes.clone()
        };
        assert_eq!(
            list_member(&network, "ipPools"),
            expected_pools,
            "case {case:?}: expected the stored ipPools, got {:?}",
            network["ipPools"]
        );
        assert_eq!(
            list_member(&network, "routes"),
            expected_routes,
            "case {case:?}: expected the stored routes, got {:?}",
            network["routes"]
        );
        assert_eq!(
            network["name"],
            name.map_or(Value::Null, |name| json!(name)),
            "case {case:?}: expected the stored name, got {:?}",
            network["name"]
        );
        // Nothing has created a cluster, so no cluster is attached to this
        // network: an empty list, not an unknown one.
        assert_eq!(
            list_member(&network, "associatedClusterIds"),
            json!([]),
            "case {case:?}: expected no associated clusters, got {:?}",
            network["associatedClusterIds"]
        );
        // The `NetworkState` a created network starts in
        // (`smithy_query(query="shape", shape="NetworkState")`: CREATING |
        // CREATE_FAILED | ACTIVE | DELETING | IDLE | IN_USE | UPDATING |
        // DELETE_FAILED | DELETED): creation is asynchronous, so the caller that
        // acts straight after the create still sees the transient state.
        assert_eq!(
            network["state"],
            json!("CREATING"),
            "case {case:?}: expected a freshly created network to be CREATING, got {:?}",
            network["state"]
        );

        // Both readers report the created network while it is still CREATING.
        for (reader, read) in [
            ("DescribeNetwork", describe(&router, &id).await),
            ("ListNetworks", list(&router, "").await),
        ] {
            let (status, body) = read;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {reader} to answer 200, got {status}: {body}"
            );
            let reported = if reader == "ListNetworks" {
                let mut page = summaries(case, &body);
                assert_eq!(
                    page.len(),
                    1,
                    "case {case:?}: expected the one stored network, got {body}"
                );
                page.remove(0)
            } else {
                body
            };
            assert_eq!(
                reported, network,
                "case {case:?}: expected {reader} to report the created network verbatim"
            );
        }

        wait_for_the_clock();

        // Once the creation has come due the network is ACTIVE, and both readers
        // agree on that too: there is one stored record, not one per operation.
        let (status, described) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described["state"],
            json!("ACTIVE"),
            "case {case:?}: expected the creation to have settled, got {:?}",
            described["state"]
        );
        let (status, listed) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListNetworks to answer 200, got {status}: {listed}"
        );
        let summary = summaries(case, &listed).remove(0);
        assert_eq!(
            summary, described,
            "case {case:?}: expected the listed summary to equal what DescribeNetwork reports"
        );
        // And every other member survived the transition unchanged.
        for member in ["arn", "id", "name", "ipPools", "routes", "associatedClusterIds"] {
            assert_eq!(
                described.get(member).cloned().unwrap_or(Value::Null),
                network.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected {member} to be unchanged by the transition, got {:?}",
                described.get(member)
            );
        }
    }
}

/// Rule: a `RequestId` (`@idempotencyToken`) that already created a network makes
/// the retry answer with that same network instead of minting a second one, while
/// a different token mints a new one.
///
/// The rows differ along the token's form and along what else the retry changes,
/// because the idempotency is keyed on the token alone.
#[tokio::test]
async fn a_retried_create_answers_with_the_network_the_token_created() {
    let router = medialive();

    // (case, requestId, the name the first create used)
    let table: [(&str, &str, &str); 3] = [
        ("a plain token", "request-1", "first"),
        (
            "a uuid-shaped token",
            "6f1b2d0e-9a62-4a3e-9c3e-1f2a3b4c5d6e",
            "second",
        ),
        ("a token of one character", "x", "third"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, token, name) in table {
        let first = created(
            &router,
            case,
            json!({
                "name": name,
                "requestId": token,
                "ipPools": [{ "cidr": "192.0.2.0/24" }],
            }),
        )
        .await;
        let id = id_of(case, &first);
        minted.push(id.clone());

        // The retry carries the same token and a different body; the token is
        // what decides, so it answers with the network already created.
        let retried = created(
            &router,
            case,
            json!({
                "name": format!("{name}-retried"),
                "requestId": token,
                "ipPools": [{ "cidr": "10.0.0.0/8" }],
            }),
        )
        .await;
        assert_eq!(
            id_of(case, &retried),
            id,
            "case {case:?}: expected the retry to answer with the network the token created"
        );
        assert_eq!(
            retried["name"],
            json!(name),
            "case {case:?}: expected the retry to report the STORED name, got {:?}",
            retried["name"]
        );
        assert_eq!(
            retried["ipPools"],
            json!([{ "cidr": "192.0.2.0/24" }]),
            "case {case:?}: expected the retry to report the STORED pools, got {:?}",
            retried["ipPools"]
        );
    }

    // One network per distinct token, and the listing is the proof: three
    // creates, three retries, three networks.
    let (ids, pages) = every_page(&router, "distinct tokens", None).await;
    assert_eq!(
        ids, minted,
        "expected one network per distinct token; pages were {pages:?}"
    );
}

/// Rule: a request the service cannot satisfy is refused with the modelled error
/// its class carries and changes nothing — a non-positive `MaxResults` and an
/// `IpPools` or `Routes` entry that is not a valid IPv4 CIDR (or a gateway that
/// is not an IPv4 address) are `BadRequestException` (400), a `Name` the account
/// already uses is `ConflictException` (409), and a `NetworkId` naming no stored
/// network is `NotFoundException` (404).
///
/// The rows differ along WHICH member is out of bounds and HOW it fails: a prefix
/// past the published /32 bound, a missing prefix, a non-numeric one, a
/// non-address, an absent `Cidr`, a list longer than the service stores, and a
/// network id that was never minted.
#[tokio::test]
async fn a_request_the_service_cannot_satisfy_is_refused() {
    let router = medialive();
    let first = created(
        &router,
        "the bystander",
        json!({ "name": "taken", "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
    )
    .await;
    let bystander = id_of("the bystander", &first);

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

    // A CIDR list the service cannot store. Each row is one way a CIDR block or
    // a gateway address fails, including the first prefix length past the
    // published /32 bound of an IPv4 prefix.
    let too_many = (0..=MAX_NETWORK_CIDR_ENTRIES)
        .map(|index| json!({ "cidr": format!("10.{index}.0.0/16") }))
        .collect::<Vec<_>>();
    let bad_requests: [(&str, Value); 10] = [
        (
            "a prefix past /32",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/33" }] }),
        ),
        (
            "no prefix at all",
            json!({ "ipPools": [{ "cidr": "192.0.2.0" }] }),
        ),
        (
            "a non-numeric prefix",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/twenty" }] }),
        ),
        (
            "not an address",
            json!({ "ipPools": [{ "cidr": "not-an-address/24" }] }),
        ),
        (
            "an IPv6 pool",
            json!({ "ipPools": [{ "cidr": "2001:db8::/32" }] }),
        ),
        ("a pool with no cidr", json!({ "ipPools": [{}] })),
        (
            "one good pool and one bad",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/24" }, { "cidr": "192.0.2.0/99" }] }),
        ),
        ("a route with no cidr", json!({ "routes": [{}] })),
        (
            "a route gateway that is not an address",
            json!({ "routes": [{ "cidr": "0.0.0.0/0", "gateway": "the-router" }] }),
        ),
        ("more pools than the service stores", json!({ "ipPools": too_many })),
    ];

    for (case, mut body) in bad_requests {
        body["name"] = json!(format!("refused-{}", case.replace(' ', "-")));
        let (status, reported) = create(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {body} to be refused, got {status}: {reported}"
        );
        assert_eq!(
            error_code(status, &reported),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {reported}"
        );
    }

    // A name the account already uses: `Name` is documented as unique in the
    // account and case-sensitive, so the same spelling conflicts and a different
    // case does not.
    let (status, reported) = create(&router, json!({ "name": "taken" })).await;
    assert_eq!(
        status, 409,
        "expected a second network under a taken name to be refused, got {status}: {reported}"
    );
    assert_eq!(
        error_code(status, &reported),
        "ConflictException",
        "expected ConflictException, got {reported}"
    );
    let differently_cased = created(&router, "a differently cased name", json!({ "name": "TAKEN" })).await;
    assert_ne!(
        id_of("a differently cased name", &differently_cased),
        bystander,
        "expected a case-sensitive name check to mint a second network"
    );

    // A network id nothing minted. Each resource type's ids come from a range of
    // its own, so another type's id (a channel's, created here) names no network
    // either.
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
    for case in ["9999999", channel_id.as_str(), "not-an-id"] {
        let (status, body) = describe(&router, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a network id nothing minted, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(case),
            "case {case:?}: expected the refusal to name the id, got {body}"
        );
    }

    // Every refusal above left the store exactly as it was: the bystander and
    // the differently cased network the two successful creates minted.
    let (ids, pages) = every_page(&router, "after the refusals", None).await;
    assert_eq!(
        ids.len(),
        2,
        "expected the refused requests to have stored nothing; got {ids:?} over {pages:?}"
    );
    assert!(
        ids.contains(&bystander),
        "expected the bystander {bystander} to still be stored, got {ids:?}"
    );
}
