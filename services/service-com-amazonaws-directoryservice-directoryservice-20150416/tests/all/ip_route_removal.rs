// SPDX-License-Identifier: CC0-1.0

//! `RemoveIpRoutes` — the blocks a directory stops routing — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body. The
//! harness (`call`, `ok`, `refused`, `create_directory`) is the one
//! `directory_lifecycle.rs` defines, and what a directory routes is always read back
//! through `ListIpRoutes`, so nothing a RESPONSE claims is read from the state struct and
//! an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a removal takes exactly the blocks the request names out of the directory, in
//!   either address family and whichever member carries them, by putting each into the
//!   modelled transient `Removing` status that the clock then takes out of the store —
//!   over rows differing in family, in which member names the blocks, in how many of the
//!   directory's blocks one call names, and in whether a block is named more than once;
//! * a request that names a block the directory does not route is refused with the
//!   modelled "specified entity could not be found" and removes nothing at all, so a
//!   partly-valid request leaves every stored block where it was;
//! * dropping a block needs a directory in service, exactly as adding one does;
//! * a request that names no block, or more blocks than any directory may route, is
//!   refused as the invalid parameter it is before the directory is resolved;
//! * the removal is idempotent while the clock still owes it: a repeat inside the span
//!   is accepted and keeps the same pending removal, and once the block is gone the same
//!   call is refused.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::MAX_IP_ROUTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, PAST_ONE_SPAN,
};
use crate::directory_networking_and_settings::{
    active_directory, emulator, routes_of, ABSENT_DIRECTORY_IDS,
};

/// The IPv4 blocks every directory in the removal table is given before anything is
/// removed. They differ in prefix length across what the modelled `CidrIp` pattern
/// admits (`smithy_query(query="shape", shape="CidrIp")` — `/1` through `/32`).
const SEEDED_IPV4: [&str; 3] = ["198.51.100.0/24", "203.0.113.7/32", "192.0.2.0/25"];

/// The IPv6 blocks every directory in the removal table is given, from the modelled
/// `CidrIpv6` pattern (`smithy_query(query="shape", shape="CidrIpv6")`).
const SEEDED_IPV6: [&str; 2] = ["2001:db8::/32", "fd00::/8"];

/// The `IpRouteStatusMsg` a block that is in service carries: the `Added` value of the
/// modelled enum (`smithy_query(query="shape", shape="IpRouteStatusMsg")` lists
/// `["Adding", "Added", "Removing", "Removed", "AddFailed", "RemoveFailed"]`).
const STATUS_ADDED: &str = "Added";

/// The `IpRouteStatusMsg` a block whose removal the clock still owes carries: the
/// `Removing` value of the same modelled enum.
const STATUS_REMOVING: &str = "Removing";

/// One row of a refusal table: the case, and the two optional members the request
/// carries (`None` for a member the request does not send at all).
type RefusalRow<'a> = (&'a str, Option<&'a [&'a str]>, Option<&'a [&'a str]>);

/// One row of the removal table: the members the request carries, and the blocks the
/// directory must stop routing because of it.
struct RemovalCase {
    name: &'static str,
    cidr_ips: Option<&'static [&'static str]>,
    cidr_ipv6s: Option<&'static [&'static str]>,
    removed: &'static [&'static str],
}

/// A `RemoveIpRoutes` body that carries only the members a row names: an omitted member
/// is one the request does not send at all.
fn removal_body(
    directory_id: &str,
    cidr_ips: Option<&[&str]>,
    cidr_ipv6s: Option<&[&str]>,
) -> Value {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(blocks) = cidr_ips {
        body["CidrIps"] = json!(blocks);
    }
    if let Some(blocks) = cidr_ipv6s {
        body["CidrIpv6s"] = json!(blocks);
    }
    body
}

/// Give `directory_id` every seeded block, over HTTP.
async fn seed_routes(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let routes: Vec<Value> = SEEDED_IPV4
        .iter()
        .map(|block| json!({ "CidrIp": block }))
        .chain(SEEDED_IPV6.iter().map(|block| json!({ "CidrIpv6": block })))
        .collect();
    ok(
        router,
        "AddIpRoutes",
        json!({ "DirectoryId": directory_id, "IpRoutes": routes }),
        case,
    )
    .await;
}

/// Every seeded block, as a set.
fn seeded_blocks() -> BTreeSet<String> {
    SEEDED_IPV4
        .iter()
        .chain(SEEDED_IPV6.iter())
        .map(|block| (*block).to_string())
        .collect()
}

#[tokio::test]
async fn a_removal_takes_exactly_the_named_blocks_out_of_the_store() {
    let (router, _engine) = emulator();

    // The rows differ along everything the request can differ along: the family of the
    // blocks, which member carries them, how many of the directory's blocks one call
    // names, and whether a block is named more than once.
    let cases = [
        RemovalCase {
            name: "one IPv4 block of several",
            cidr_ips: Some(&["198.51.100.0/24"]),
            cidr_ipv6s: None,
            removed: &["198.51.100.0/24"],
        },
        RemovalCase {
            name: "a host route, the longest IPv4 prefix",
            cidr_ips: Some(&["203.0.113.7/32"]),
            cidr_ipv6s: None,
            removed: &["203.0.113.7/32"],
        },
        RemovalCase {
            name: "every IPv4 block in one call",
            cidr_ips: Some(&SEEDED_IPV4),
            cidr_ipv6s: None,
            removed: &SEEDED_IPV4,
        },
        RemovalCase {
            name: "one IPv6 block, named in its own member",
            cidr_ips: None,
            cidr_ipv6s: Some(&["2001:db8::/32"]),
            removed: &["2001:db8::/32"],
        },
        RemovalCase {
            name: "both families in one call",
            cidr_ips: Some(&["192.0.2.0/25"]),
            cidr_ipv6s: Some(&["fd00::/8"]),
            removed: &["192.0.2.0/25", "fd00::/8"],
        },
        RemovalCase {
            name: "a block named twice in one call",
            cidr_ips: Some(&["198.51.100.0/24", "198.51.100.0/24"]),
            cidr_ipv6s: None,
            removed: &["198.51.100.0/24"],
        },
        RemovalCase {
            name: "every block the directory routes",
            cidr_ips: Some(&SEEDED_IPV4),
            cidr_ipv6s: Some(&SEEDED_IPV6),
            removed: &["198.51.100.0/24", "203.0.113.7/32", "192.0.2.0/25", "2001:db8::/32", "fd00::/8"],
        },
    ];

    // Every row gets its own directory, and they are all created before anything waits:
    // the stage transition each create owes falls due on the clock, so one wait covers
    // the whole table.
    let mut ids: Vec<String> = Vec::new();
    for (row, case) in cases.iter().enumerate() {
        ids.push(
            create_directory(
                &router,
                minimal_create(&format!("removal-{row}.example.com")),
                case.name,
            )
            .await,
        );
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The same blocks go into every directory over HTTP, then one wait puts them all in
    // service, so what a removal changes is visible against a known starting point.
    for (case, id) in cases.iter().zip(&ids) {
        seed_routes(&router, id, case.name).await;
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (case, id) in cases.iter().zip(&ids) {
        let before = routes_of(&router, id, case.name).await;
        let before_blocks: BTreeSet<String> = before.keys().cloned().collect();
        assert_eq!(
            before_blocks,
            seeded_blocks(),
            "case {:?}: the directory routes every seeded block before the removal, \
             got {before_blocks:?}",
            case.name
        );

        ok(
            &router,
            "RemoveIpRoutes",
            removal_body(id, case.cidr_ips, case.cidr_ipv6s),
            case.name,
        )
        .await;

        // The block is not out of service the instant the call answers: it carries the
        // modelled transient status until the clock-owed removal falls due, while the
        // blocks the request did not name are untouched and still in service.
        let pending = routes_of(&router, id, case.name).await;
        let removed: BTreeSet<String> = case.removed.iter().map(|b| (*b).to_string()).collect();
        let pending_blocks: BTreeSet<String> = pending.keys().cloned().collect();
        assert_eq!(
            pending_blocks,
            seeded_blocks(),
            "case {:?}: a block still being removed is still reported, got {pending_blocks:?}",
            case.name
        );
        for (block, info) in &pending {
            let expected = if removed.contains(block) {
                STATUS_REMOVING
            } else {
                STATUS_ADDED
            };
            assert_eq!(
                info["IpRouteStatusMsg"],
                json!(expected),
                "case {:?}: block {block} reports {expected}, got {info}",
                case.name
            );
        }

        // Nothing but time passes here: the removal falls due on the clock, not on a
        // later call.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let after = routes_of(&router, id, case.name).await;
        let after_blocks: BTreeSet<String> = after.keys().cloned().collect();
        let expected_left: BTreeSet<String> = seeded_blocks()
            .into_iter()
            .filter(|block| !removed.contains(block))
            .collect();
        assert_eq!(
            after_blocks, expected_left,
            "case {:?}: the directory routes exactly the blocks it was left with, \
             got {after_blocks:?}",
            case.name
        );
        for (block, info) in &after {
            assert_eq!(
                info["IpRouteStatusMsg"],
                json!(STATUS_ADDED),
                "case {:?}: block {block} the removal did not name stays in service, \
                 got {info}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_block_the_directory_does_not_route_is_refused_and_removes_nothing() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "absent blocks").await;
    seed_routes(&router, &id, "absent blocks").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // The rows differ in WHY no such route exists: a block of either family that was
    // never added, a block named beside one the directory does route (so a refusal that
    // removed the valid half would be caught), and a block that was removed already.
    let gone = "10.10.0.0/16";
    ok(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": gone }] }),
        "absent blocks",
    )
    .await;
    ok(
        &router,
        "RemoveIpRoutes",
        json!({ "DirectoryId": id, "CidrIps": [gone] }),
        "absent blocks",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let cases: [RefusalRow<'_>; 5] = [
        ("an IPv4 block never added", Some(&["172.16.0.0/12"]), None),
        ("an IPv6 block never added", None, Some(&["fe80::/64"])),
        (
            "an absent block beside one the directory routes",
            Some(&["198.51.100.0/24", "172.31.0.0/16"]),
            None,
        ),
        (
            "an absent IPv6 block beside an IPv4 block it routes",
            Some(&["203.0.113.7/32"]),
            Some(&["2001:db8:1::/48"]),
        ),
        ("a block already removed", Some(&[gone]), None),
    ];

    let before = routes_of(&router, &id, "absent blocks").await;
    for (case, cidr_ips, cidr_ipv6s) in cases {
        refused(
            &router,
            "RemoveIpRoutes",
            removal_body(&id, cidr_ips, cidr_ipv6s),
            "EntityDoesNotExistException",
            case,
        )
        .await;
        let after = routes_of(&router, &id, case).await;
        let before_blocks: BTreeSet<&String> = before.keys().collect();
        let after_blocks: BTreeSet<&String> = after.keys().collect();
        assert_eq!(
            after_blocks, before_blocks,
            "case {case:?}: a refused removal leaves the stored blocks untouched, \
             got {after_blocks:?}"
        );
        for (block, info) in &after {
            assert_eq!(
                info["IpRouteStatusMsg"],
                json!(STATUS_ADDED),
                "case {case:?}: block {block} is left in service, got {info}"
            );
        }
    }

    // A directory no id is stored under is refused the same way, over a table of
    // well-formed ids.
    for (absent, class) in ABSENT_DIRECTORY_IDS {
        refused(
            &router,
            "RemoveIpRoutes",
            json!({ "DirectoryId": absent, "CidrIps": ["198.51.100.0/24"] }),
            "EntityDoesNotExistException",
            &format!("RemoveIpRoutes on {class} {absent}"),
        )
        .await;
    }
}

#[tokio::test]
async fn dropping_a_block_needs_a_directory_that_is_in_service() {
    let (router, _engine) = emulator();
    // The create answers while the directory is still in the first stage of the modelled
    // `DirectoryStage` enum, so a caller acting immediately after it sees the transient
    // state and the modelled refusal that goes with it — whatever the request names.
    let id = create_directory(
        &router,
        minimal_create("corp.example.com"),
        "a directory still being built",
    )
    .await;
    for (case, cidr_ips, cidr_ipv6s) in [
        (
            "an IPv4 block, directory still being built",
            Some(&["198.51.100.0/24"][..]),
            None,
        ),
        (
            "an IPv6 block, directory still being built",
            None,
            Some(&["2001:db8::/32"][..]),
        ),
    ] {
        refused(
            &router,
            "RemoveIpRoutes",
            removal_body(&id, cidr_ips, cidr_ipv6s),
            "DirectoryUnavailableException",
            case,
        )
        .await;
    }

    // Once the transition the create owes has fallen due, the same call resolves the
    // route instead, and a block the directory routes is removed.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }] }),
        "a directory in service",
    )
    .await;
    ok(
        &router,
        "RemoveIpRoutes",
        json!({ "DirectoryId": id, "CidrIps": ["198.51.100.0/24"] }),
        "a directory in service",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let stored = routes_of(&router, &id, "a directory in service").await;
    assert!(
        stored.is_empty(),
        "case {:?}: the removed block is gone, got {stored:?}",
        "a directory in service"
    );
}

#[tokio::test]
async fn a_request_that_names_no_single_block_is_refused_as_an_invalid_parameter() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "malformed removals").await;
    seed_routes(&router, &id, "malformed removals").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Both members are optional in the model (`CidrIps` carries the documented default
    // `[]`), so a request may reach the handler naming no block at all — and such a
    // request asks for no removal whatever the directory routes. A request longer than
    // the published 100-block bound cannot name only blocks a directory routes either.
    let too_many: Vec<String> = (0..=MAX_IP_ROUTES).map(|i| format!("10.{i}.0.0/16")).collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    let cases: [RefusalRow<'_>; 5] = [
        ("neither member sent", None, None),
        ("an empty CidrIps", Some(&[]), None),
        ("an empty CidrIpv6s", None, Some(&[])),
        ("both members empty", Some(&[]), Some(&[])),
        ("one block past the published bound", Some(&too_many), None),
    ];
    for (case, cidr_ips, cidr_ipv6s) in cases {
        refused(
            &router,
            "RemoveIpRoutes",
            removal_body(&id, cidr_ips, cidr_ipv6s),
            "InvalidParameterException",
            case,
        )
        .await;
        let after = routes_of(&router, &id, case).await;
        let after_blocks: BTreeSet<String> = after.keys().cloned().collect();
        assert_eq!(
            after_blocks,
            seeded_blocks(),
            "case {case:?}: a refused removal leaves every stored block in place, \
             got {after_blocks:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_removal_is_accepted_while_the_clock_still_owes_it() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "repeat removals").await;
    let block = "198.51.100.0/24";
    ok(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": block }] }),
        "repeat removals",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let body = json!({ "DirectoryId": id, "CidrIps": [block] });
    ok(&router, "RemoveIpRoutes", body.clone(), "repeat removals").await;
    // A repeat inside the span names a block that is still being removed, so it is
    // answered the same way and the pending removal is not restarted.
    let (status, parsed) = call(&router, "RemoveIpRoutes", body.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: a repeat inside the span is accepted, got {status} with {parsed}",
        "repeat removals"
    );
    let pending = routes_of(&router, &id, "repeat removals").await;
    assert_eq!(
        pending[block]["IpRouteStatusMsg"],
        json!(STATUS_REMOVING),
        "case {:?}: the block is still being removed, got {:?}",
        "repeat removals",
        pending[block]
    );

    // The repeat left the pending removal where it was: the block is gone once the span
    // the first removal started has elapsed.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let after = routes_of(&router, &id, "repeat removals").await;
    assert!(
        after.is_empty(),
        "case {:?}: the block is gone once the clock says so, got {after:?}",
        "repeat removals"
    );
    // And a block that is gone is one the directory does not route.
    refused(
        &router,
        "RemoveIpRoutes",
        body,
        "EntityDoesNotExistException",
        "a removal repeated after the span",
    )
    .await;
}
