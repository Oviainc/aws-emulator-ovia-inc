// SPDX-License-Identifier: CC0-1.0

//! `UpdateNetwork` — the settings-change half of the MediaLive Anywhere
//! network's lifecycle — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour, not an internal function's return value.
//!
//! The create and read side of the resource lives in `networks` and the deletion
//! in `network_deletion`, whose request helpers these suites reuse so the suites
//! cannot disagree about how a network request reaches the emulator.
//!
//! The suites are TABLES: one `UpdateNetwork` renaming a created network is ONE
//! row of the first of them.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    already_deleted_message, not_found_message, too_many_network_entries_message,
    INVALID_IP_POOL_CIDR_MESSAGE, INVALID_ROUTE_MESSAGE, MAX_NETWORK_CIDR_ENTRIES,
    NAME_ALREADY_IN_USE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::networks::{
    created, describe, id_of, list, list_member, summaries, STRAY_NETWORK_ID, NETWORKS,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateNetwork` on `id` with `body` (`smithy_query(query="operation",
/// operation="UpdateNetwork")`: PUT /prod/networks/{NetworkId}).
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("{NETWORKS}/{id}"), Some(body)).await
}

/// The three members an `UpdateNetwork` request can change
/// (`smithy_query(query="operation", operation="UpdateNetwork")`: `Name`,
/// `IpPools`, `Routes` beside the `NetworkId` label).
const CHANGEABLE: [&str; 3] = ["name", "ipPools", "routes"];

/// The members no `UpdateNetwork` request carries, so no update can move them.
const UNCHANGEABLE: [&str; 3] = ["arn", "id", "associatedClusterIds"];

/// The one network `ListNetworks` reports, asserted to be the only one stored.
async fn only_listed(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = list(router, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListNetworks to answer 200, got {status}: {body}"
    );
    let mut page = summaries(case, &body);
    assert_eq!(
        page.len(),
        1,
        "case {case:?}: expected the one stored network, got {body}"
    );
    page.remove(0)
}

/// The network `id` names, read back through its own reader.
async fn read_back(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = describe(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeNetwork to answer 200, got {status}: {body}"
    );
    body
}

/// `MAX_NETWORK_CIDR_ENTRIES` pool entries — the most this emulator stores on one
/// network — and one more than that, which is the first list it refuses.
fn pools(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "cidr": format!("10.{}.{}.0/24", index / 256, index % 256) }))
            .collect(),
    )
}

/// The same list as `routes` entries, each with a gateway inside its own block.
fn routes(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "cidr": format!("10.{}.{}.0/24", index / 256, index % 256),
                    "gateway": format!("10.{}.{}.1", index / 256, index % 256),
                })
            })
            .collect(),
    )
}

/// One row of the update table: what the row creates, when it updates and what
/// the update asks to change.
struct Update {
    case: &'static str,
    /// The `CreateNetwork` body the row stores first.
    create: fn() -> Value,
    /// Whether the create's transient `CREATING` is allowed to settle into
    /// `ACTIVE` before the update is called.
    settle_before_update: bool,
    /// The `UpdateNetwork` body the row sends.
    change: fn() -> Value,
}

/// Rule: `UpdateNetwork` replaces exactly the settings its request names and
/// leaves every other one as it was — "Include this parameter only if you want to
/// change …" on each of `Name`, `IpPools` and `Routes` — then reports the stored
/// network, whose `State` the change puts in `UPDATING` with the clock owing it
/// `ACTIVE` again, because `State` is "The current state of the Network. Only
/// MediaLive Anywhere can change the state". The members no update request
/// carries (the id, the ARN and the derived cluster association) cannot move, and
/// both readers go on reporting the one stored record.
///
/// The rows differ along WHICH members the update names, along the SIZE and FORM
/// of the two CIDR lists it names (absent, empty, one entry, several, a route
/// with a gateway and one without, the published prefix bounds, the most entries
/// the service stores) and along WHEN the update is called (inside the transient
/// `CREATING` window and after it has settled), because that is what the rule
/// generalises over. What each row expects is read off its two REQUESTS and the
/// network the create reported, never off the emulator.
#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest_alone() {
    let table = [
        Update {
            case: "a rename only, while the create is still CREATING",
            create: || {
                json!({
                    "name": "alpha",
                    "ipPools": [{ "cidr": "192.0.2.0/24" }],
                    "routes": [{ "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" }],
                })
            },
            settle_before_update: false,
            change: || json!({ "name": "alpha-renamed" }),
        },
        Update {
            case: "a rename only, once the network is ACTIVE",
            create: || {
                json!({
                    "name": "beta",
                    "ipPools": [{ "cidr": "192.0.2.0/24" }],
                    "routes": [{ "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" }],
                })
            },
            settle_before_update: true,
            change: || json!({ "name": "beta-renamed" }),
        },
        Update {
            case: "the same name re-sent",
            create: || json!({ "name": "unchanged-name" }),
            settle_before_update: true,
            change: || json!({ "name": "unchanged-name" }),
        },
        Update {
            case: "two pools for one",
            create: || json!({ "name": "pools", "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
            settle_before_update: true,
            change: || {
                json!({
                    "ipPools": [
                        { "cidr": "198.51.100.128/25" },
                        { "cidr": "172.16.0.0/12" },
                    ],
                })
            },
        },
        Update {
            case: "the routes replaced, one with a gateway and one without",
            create: || {
                json!({
                    "name": "routes",
                    "routes": [{ "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" }],
                })
            },
            settle_before_update: true,
            change: || {
                json!({
                    "routes": [
                        { "cidr": "203.0.113.0/24", "gateway": "203.0.113.1" },
                        { "cidr": "198.51.100.0/24" },
                    ],
                })
            },
        },
        Update {
            case: "the pools cleared with an empty list",
            create: || {
                json!({
                    "name": "cleared",
                    "ipPools": [{ "cidr": "192.0.2.0/24" }, { "cidr": "10.0.0.0/8" }],
                    "routes": [{ "cidr": "0.0.0.0/0" }],
                })
            },
            settle_before_update: true,
            change: || json!({ "ipPools": [] }),
        },
        Update {
            case: "pools added to a network created without any",
            create: || json!({ "name": "bare" }),
            settle_before_update: true,
            change: || json!({ "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
        },
        Update {
            case: "every changeable member at once",
            create: || {
                json!({
                    "name": "everything",
                    "ipPools": [{ "cidr": "192.0.2.0/24" }],
                    "routes": [{ "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" }],
                })
            },
            settle_before_update: true,
            change: || {
                json!({
                    "name": "everything-else",
                    "ipPools": [{ "cidr": "10.0.0.0/8" }, { "cidr": "172.16.0.0/12" }],
                    "routes": [{ "cidr": "10.0.0.0/8", "gateway": "10.0.0.1" }],
                })
            },
        },
        Update {
            case: "nothing named at all",
            create: || {
                json!({
                    "name": "untouched",
                    "ipPools": [{ "cidr": "192.0.2.0/24" }],
                    "routes": [{ "cidr": "0.0.0.0/0" }],
                })
            },
            settle_before_update: true,
            change: || json!({}),
        },
        Update {
            case: "the published prefix bounds",
            create: || json!({ "name": "bounds" }),
            settle_before_update: true,
            // The bounds of an IPv4 prefix length: /0 and /32.
            change: || {
                json!({
                    "ipPools": [{ "cidr": "0.0.0.0/0" }, { "cidr": "192.0.2.7/32" }],
                    "routes": [{ "cidr": "192.0.2.7/32", "gateway": "192.0.2.1" }],
                })
            },
        },
        Update {
            case: "the most pools and routes the service stores",
            create: || json!({ "name": "full-lists" }),
            settle_before_update: true,
            change: || {
                json!({
                    "ipPools": pools(MAX_NETWORK_CIDR_ENTRIES),
                    "routes": routes(MAX_NETWORK_CIDR_ENTRIES),
                })
            },
        },
    ];

    for Update {
        case,
        create,
        settle_before_update,
        change,
    } in table
    {
        let router = medialive();
        let network = created(&router, case, create()).await;
        let id = id_of(case, &network);

        if settle_before_update {
            wait_for_the_clock();
        }

        // What the network looks like one instant before the update, read back
        // out of the store through its own reader.
        let before = read_back(&router, case, &id).await;
        let state_before = if settle_before_update {
            "ACTIVE"
        } else {
            "CREATING"
        };
        assert_eq!(
            before["state"],
            json!(state_before),
            "case {case:?}: expected the network to be {state_before} before the update, got {:?}",
            before["state"]
        );

        let request = change();
        let (status, answered) = update(&router, &id, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateNetwork to answer 200, got {status}: {answered}"
        );

        // Each member the request named now reads as the request named it; each
        // one it omitted reads exactly as it did before the update.
        for member in CHANGEABLE {
            let expected = match request.get(member) {
                Some(value) => value.clone(),
                None => before.get(member).cloned().unwrap_or(Value::Null),
            };
            let actual = if member == "name" {
                answered.get(member).cloned().unwrap_or(Value::Null)
            } else {
                list_member(&answered, member)
            };
            let expected = if member == "name" {
                expected
            } else {
                match expected {
                    Value::Null => json!([]),
                    other => other,
                }
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} to read as {expected}, got {actual}"
            );
        }
        for member in UNCHANGEABLE {
            assert_eq!(
                answered.get(member).cloned().unwrap_or(Value::Null),
                before.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected {member} to be unchanged by the update, got {:?}",
                answered.get(member)
            );
        }
        // Applying a network's settings is asynchronous, so a caller that acts
        // straight after the update still sees the transient state the clock owes
        // `ACTIVE`.
        assert_eq!(
            answered["state"],
            json!("UPDATING"),
            "case {case:?}: expected the update to report UPDATING, got {:?}",
            answered["state"]
        );

        // Both readers answer from the one stored record, so each reports exactly
        // what the update just answered.
        assert_eq!(
            read_back(&router, case, &id).await,
            answered,
            "case {case:?}: expected DescribeNetwork to report what the update did"
        );
        assert_eq!(
            only_listed(&router, case).await,
            answered,
            "case {case:?}: expected the listing to report what the update did"
        );

        wait_for_the_clock();

        // The change has come due: the network is `ACTIVE` again and every other
        // member survived the transition exactly as the update stored it.
        let after = read_back(&router, case, &id).await;
        assert_eq!(
            after["state"],
            json!("ACTIVE"),
            "case {case:?}: expected the update to have settled, got {:?}",
            after["state"]
        );
        for member in CHANGEABLE.into_iter().chain(UNCHANGEABLE) {
            assert_eq!(
                after.get(member).cloned().unwrap_or(Value::Null),
                answered.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected {member} to be unchanged by the transition, got {:?}",
                after.get(member)
            );
        }
    }
}

/// Rule: an `UpdateNetwork` the service cannot satisfy is refused with the
/// modelled error its class carries and writes NOTHING — an `IpPools` or `Routes`
/// list this emulator cannot store is `BadRequestException` (400) worded exactly
/// as `CreateNetwork` refuses the same list, and a `Name` another live network of
/// the account holds is `ConflictException` (409), because `Name` is "unique in
/// the AWS account".
///
/// The rows differ along WHICH member is out of bounds and HOW it fails: a prefix
/// past the published /32 bound, a missing prefix, a non-numeric one, a
/// non-address, an IPv6 block, an absent `Cidr`, a gateway that is not an
/// address, a list one entry longer than the service stores, and a name the
/// account already uses.
#[tokio::test]
async fn an_update_the_service_cannot_satisfy_is_refused() {
    let router = medialive();
    let subject = created(
        &router,
        "the subject",
        json!({
            "name": "subject",
            "ipPools": [{ "cidr": "192.0.2.0/24" }],
            "routes": [{ "cidr": "0.0.0.0/0", "gateway": "192.0.2.1" }],
        }),
    )
    .await;
    let id = id_of("the subject", &subject);
    // A second live network, so a row can ask for a name the account holds.
    created(&router, "the namesake", json!({ "name": "taken" })).await;
    wait_for_the_clock();

    let before = read_back(&router, "before the refusals", &id).await;
    assert_eq!(
        before["state"],
        json!("ACTIVE"),
        "expected the subject to be ACTIVE before the refusals, got {:?}",
        before["state"]
    );

    // (case, the refused body, the status, the error code, the message)
    let table: [(&str, Value, u16, &str, String); 12] = [
        (
            "a prefix past /32",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/33" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "no prefix at all",
            json!({ "ipPools": [{ "cidr": "192.0.2.0" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "a non-numeric prefix",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/twenty" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "not an address",
            json!({ "ipPools": [{ "cidr": "not-an-address/24" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "an IPv6 pool",
            json!({ "ipPools": [{ "cidr": "2001:db8::/32" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "a pool with no cidr",
            json!({ "ipPools": [{}] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "one good pool and one bad",
            json!({ "ipPools": [{ "cidr": "192.0.2.0/24" }, { "cidr": "192.0.2.0/99" }] }),
            400,
            "BadRequestException",
            INVALID_IP_POOL_CIDR_MESSAGE.to_string(),
        ),
        (
            "a route with no cidr",
            json!({ "routes": [{}] }),
            400,
            "BadRequestException",
            INVALID_ROUTE_MESSAGE.to_string(),
        ),
        (
            "a route gateway that is not an address",
            json!({ "routes": [{ "cidr": "0.0.0.0/0", "gateway": "the-router" }] }),
            400,
            "BadRequestException",
            INVALID_ROUTE_MESSAGE.to_string(),
        ),
        (
            "one pool more than the service stores",
            json!({ "ipPools": pools(MAX_NETWORK_CIDR_ENTRIES + 1) }),
            400,
            "BadRequestException",
            too_many_network_entries_message("ipPools"),
        ),
        (
            "one route more than the service stores",
            json!({ "routes": routes(MAX_NETWORK_CIDR_ENTRIES + 1) }),
            400,
            "BadRequestException",
            too_many_network_entries_message("routes"),
        ),
        (
            "a name another live network holds",
            json!({ "name": "taken" }),
            409,
            "ConflictException",
            NAME_ALREADY_IN_USE_MESSAGE.to_string(),
        ),
    ];

    for (case, body, status_wanted, code, message) in table {
        let (status, refusal) = update(&router, &id, body).await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: expected {status_wanted}, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            code,
            "case {case:?}: expected {code}, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            message,
            "case {case:?}: expected the shared wording of its rejection class, got {:?}",
            error_message(&refusal)
        );
        // The refusal was taken before anything was written, so the network is
        // still exactly what it was — its lifecycle member included, which no
        // refused update may move into `UPDATING`.
        assert_eq!(
            read_back(&router, case, &id).await,
            before,
            "case {case:?}: expected the refused update to leave the network alone"
        );
    }
}

/// Rule: an `UpdateNetwork` whose `NetworkId` names no network a caller may
/// change — one no network was ever minted under, or one whose delete this
/// service has accepted — is refused and writes nothing. This operation models no
/// `NotFoundException` (`smithy_query(query="operation",
/// operation="UpdateNetwork")` declares BadGateway, BadRequest, Conflict,
/// Forbidden, GatewayTimeout, InternalServerError and TooManyRequests), so the
/// refusal is its modelled `BadRequestException` (400) naming the id the request
/// carried, while the readers — which do model a 404 — go on reporting the
/// deleted record.
///
/// The rows differ along the FORM of the id (one in the network range nothing
/// minted, another resource type's id, a non-numeric string) and, for a deleted
/// network, along WHEN the update is called: inside the transient `DELETING`
/// window and after the terminal `DELETED`.
#[tokio::test]
async fn an_update_of_a_network_no_caller_may_change_is_refused() {
    let router = medialive();
    let doomed = created(
        &router,
        "the doomed network",
        json!({ "name": "doomed", "ipPools": [{ "cidr": "192.0.2.0/24" }] }),
    )
    .await;
    let doomed_id = id_of("the doomed network", &doomed);

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

    for case in [STRAY_NETWORK_ID, channel_id.as_str(), "not-an-id", "-1"] {
        let (status, refusal) = update(&router, case, json!({ "name": "nowhere" })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a network id nothing minted, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            not_found_message("Network", case),
            "case {case:?}: expected the shared not-found wording, got {:?}",
            error_message(&refusal)
        );
    }

    // The same refusal once a delete has been accepted for the id, in both
    // halves of the deletion lifecycle: the record stays readable, but it is no
    // longer a network a caller may change.
    let (status, deleting) = call(
        &router,
        "DELETE",
        &format!("{NETWORKS}/{doomed_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: expected DeleteNetwork to answer 202, got {status}: {deleting}"
    );

    for (case, settle_first) in [
        ("inside the DELETING window", false),
        ("after the terminal DELETED", true),
    ] {
        if settle_first {
            wait_for_the_clock();
        }
        let before = read_back(&router, case, &doomed_id).await;
        let (status, refusal) = update(
            &router,
            &doomed_id,
            json!({ "name": "renamed-after-delete", "ipPools": [{ "cidr": "10.0.0.0/8" }] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 on a network whose delete was accepted, \
             got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            already_deleted_message(&format!("Network {doomed_id}")),
            "case {case:?}: expected the shared already-deleted wording, got {:?}",
            error_message(&refusal)
        );
        assert_eq!(
            read_back(&router, case, &doomed_id).await,
            before,
            "case {case:?}: expected the refused update to leave the deleted record alone"
        );
    }
}
