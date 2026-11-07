// SPDX-License-Identifier: CC0-1.0

//! IP routing, tag removal, snapshot limits, the LDAPS and client-authentication reads,
//! and Region removal — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is shared with
//! `directory_lifecycle`). Nothing a RESPONSE claims is read from the state struct, so
//! an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * an operation that names a directory resolves it in the store FIRST and refuses an
//!   id no directory is stored under with that operation's own modelled not-found error
//!   (`EntityDoesNotExistException` for the entity-shaped ones,
//!   `DirectoryDoesNotExistException` for the directory-shaped ones) — over a table of
//!   well-formed ids that differ in digits and form — while an input the MODEL refuses
//!   (a value outside an enum, a `Limit` outside its range) is refused before that
//!   resolution happens at all;
//! * an IP address block is persisted once per directory, under the clock-owed `Adding`
//!   status, and is reported back from the store by `ListIpRoutes` — over a table of
//!   blocks differing in family (IPv4 / IPv6), prefix length, description and count;
//! * the published bounds hold: at most 100 blocks per directory, and the manual
//!   snapshot limit of 5 with `ManualSnapshotsLimitReached` derived from the count;
//! * a reporting operation reports what the store holds and nothing else: a directory
//!   with no LDAPS or client-authentication setting recorded reports an empty list, and
//!   a seeded setting is reported member for member, filtered by type and paged;
//! * `RemoveRegion` acts on the Region the call was signed for: the primary Region is
//!   refused as unsupported, a Region the directory does not exist in is refused as
//!   not found, and a replicated Region is dropped.
//!
//! `EnableLDAPS` and `EnableClientAuthentication` are still scaffolded, and a Simple AD
//! directory does not replicate across Regions, so the settings those operations would
//! write and the replicated Regions of such a directory are seeded directly into the
//! store — the seam `state_roundtrip.rs` documents — and then read back over HTTP.
//! Everything an implemented operation can write is written over HTTP.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::{
    ClientAuthenticationSetting, Directory, LdapsSetting, RegionReplication, MAX_IP_ROUTES,
    MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY,
};
use service_com_amazonaws_directoryservice_directoryservice_20150416::ComAmazonawsDirectoryserviceDirectoryservice20150416State as DirectoryServiceState;
use smithy_server_runtime::MultiServiceRouter;

use super::directory_lifecycle::{
    call, call_in_region, create_directory, emulator_on, minimal_create, ok, refused, tag_map,
    PAST_ONE_SPAN, REGION,
};
use super::event_topics::epoch_seconds;

/// A replication of a directory to `region` that is already in service, as `AddRegion`
/// leaves one once the transition it owes has fallen due.
///
/// A Simple AD directory does not replicate across Regions, so `AddRegion` cannot write
/// this one: the record is seeded directly, the seam `state_roundtrip.rs` documents.
fn settled_replication(region: &str, now: SystemTime) -> RegionReplication {
    RegionReplication {
        region_name: region.to_string(),
        vpc_settings: None,
        desired_number_of_domain_controllers: None,
        status: "Active".to_string(),
        status_due_at: None,
        launch_time: now,
        status_last_updated: now,
    }
}

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="AddIpRoutes")`), so every row here is a
/// value the boundary validator admits and the handler must resolve in the store. The
/// first row is the id the behavioural corpus used; the rest are the same shape with
/// different digits, including the two extremes of the alphabet.
pub(crate) const ABSENT_DIRECTORY_IDS: [(&str, &str); 5] = [
    ("d-1dca5812e8", "the recorded id"),
    ("d-74acc7c03b", "another recorded id"),
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
];

/// The modelled `LDAPSStatus` catalog: `smithy_query(query="shape", shape="LDAPSStatus")`
/// answers `["Enabling", "Enabled", "EnableFailed", "Disabled"]`.
const LDAPS_STATUSES: [&str; 4] = ["Enabling", "Enabled", "EnableFailed", "Disabled"];

/// The modelled `ClientAuthenticationType` catalog:
/// `smithy_query(query="shape", shape="ClientAuthenticationType")` answers
/// `["SmartCard", "SmartCardOrPassword"]`.
const CLIENT_AUTHENTICATION_TYPES: [&str; 2] = ["SmartCard", "SmartCardOrPassword"];

/// The modelled `ClientAuthenticationStatus` catalog:
/// `smithy_query(query="shape", shape="ClientAuthenticationStatus")` answers
/// `["Enabled", "Disabled"]`.
const CLIENT_AUTHENTICATION_STATUSES: [&str; 2] = ["Enabled", "Disabled"];

/// The only value the modelled `LDAPSType` enum carries:
/// `smithy_query(query="shape", shape="LDAPSType")` answers `["Client"]`.
const LDAPS_TYPE: &str = "Client";

/// An instant well clear of the epoch, so a timestamp read back from the store can be
/// compared exactly rather than approximately.
const SEEDED_INSTANT: Duration = Duration::from_secs(1_700_000_000);

/// A router and the engine it serves from, so a test can seed the records whose writing
/// operations are still scaffolded.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(Arc::clone(&engine)), engine)
}

/// Edit one stored directory in place — the seam for a record whose writing operation is
/// still scaffolded.
fn with_directory<T>(
    engine: &Arc<engine::InMemoryEngine>,
    directory_id: &str,
    edit: impl FnOnce(&mut Directory) -> T,
) -> T {
    let state = engine.state::<DirectoryServiceState>();
    let mut store = state.lock();
    let directory = store
        .directories
        .get_mut(directory_id)
        .unwrap_or_else(|| panic!("the store holds directory {directory_id}"));
    edit(directory)
}

/// A directory created over HTTP and waited past the clock-owed stage transition, so it
/// is in the `Active` stage every mutation below needs.
pub(crate) async fn active_directory(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    active_directory_in(router, REGION, name, case).await
}

/// The same, created against `region` so that Region is the directory's primary one.
async fn active_directory_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    case: &str,
) -> String {
    let (status, body) =
        call_in_region(router, "CreateDirectory", minimal_create(name), region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateDirectory in {region} answers 200, got {status} with {body}"
    );
    let id = body["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DirectoryId, got {body}"))
        .to_string();
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// A call signed for `region` that must be refused with HTTP 400 and `expected_code`.
async fn refused_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) {
    let (status, parsed) = call_in_region(router, operation, body, region).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
}

/// The `IpRoutesInfo` of a `ListIpRoutes` answer, keyed by the address block each entry
/// routes.
pub(crate) async fn routes_of(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeMap<String, Value> {
    let answer = ok(
        router,
        "ListIpRoutes",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["IpRoutesInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected IpRoutesInfo, got {answer}"))
        .iter()
        .map(|info| {
            let block = info["CidrIp"]
                .as_str()
                .or_else(|| info["CidrIpv6"].as_str())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every route names a block, got {info}")
                })
                .to_string();
            (block, info.clone())
        })
        .collect()
}

/// One row of the IP route table: a request, and the blocks it must leave stored.
struct RouteCase {
    name: &'static str,
    routes: Value,
    blocks: &'static [&'static str],
}

#[tokio::test]
async fn ip_routes_are_persisted_once_and_reported_from_the_store() {
    let (router, _engine) = emulator();

    // The rows differ along everything an `IpRoute` can differ along: the family of the
    // block (the modelled `CidrIp` and `CidrIpv6` members), the prefix length at both
    // ends of what their patterns admit, whether a `Description` came with it, and how
    // many blocks one request carries.
    let cases = [
        RouteCase {
            name: "a single /24 with a description",
            routes: json!([{ "CidrIp": "198.51.100.0/24", "Description": "dns servers" }]),
            blocks: &["198.51.100.0/24"],
        },
        RouteCase {
            name: "a single host route, the longest IPv4 prefix",
            routes: json!([{ "CidrIp": "203.0.113.7/32" }]),
            blocks: &["203.0.113.7/32"],
        },
        RouteCase {
            name: "the shortest IPv4 prefix the pattern admits",
            routes: json!([{ "CidrIp": "10.0.0.0/1" }]),
            blocks: &["10.0.0.0/1"],
        },
        RouteCase {
            name: "two IPv4 blocks in one request",
            routes: json!([
                { "CidrIp": "192.0.2.0/25", "Description": "site a" },
                { "CidrIp": "192.0.2.128/25", "Description": "site b" }
            ]),
            blocks: &["192.0.2.0/25", "192.0.2.128/25"],
        },
        RouteCase {
            name: "an IPv6 block",
            routes: json!([{ "CidrIpv6": "2001:db8::/32", "Description": "v6 dns" }]),
            blocks: &["2001:db8::/32"],
        },
        RouteCase {
            name: "both families in one request",
            routes: json!([
                { "CidrIp": "172.16.0.0/12" },
                { "CidrIpv6": "fd00::/8" }
            ]),
            blocks: &["172.16.0.0/12", "fd00::/8"],
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
                minimal_create(&format!("routes-{row}.example.com")),
                case.name,
            )
            .await,
        );
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (case, id) in cases.iter().zip(&ids) {
        let before = routes_of(&router, id, case.name).await;
        assert!(
            before.is_empty(),
            "case {:?}: a fresh directory routes nothing, got {before:?}",
            case.name
        );

        ok(
            &router,
            "AddIpRoutes",
            json!({ "DirectoryId": id, "IpRoutes": case.routes }),
            case.name,
        )
        .await;

        let stored = routes_of(&router, id, case.name).await;
        let expected: BTreeSet<String> = case.blocks.iter().map(|b| (*b).to_string()).collect();
        let found: BTreeSet<String> = stored.keys().cloned().collect();
        assert_eq!(
            found, expected,
            "case {:?}: expected the blocks {expected:?} to be stored, got {found:?}",
            case.name
        );

        let requested = case
            .routes
            .as_array()
            .expect("the table's routes are arrays");
        for route in requested {
            let block = route["CidrIp"]
                .as_str()
                .or_else(|| route["CidrIpv6"].as_str())
                .expect("the table's routes name a block");
            let info = &stored[block];
            assert_eq!(
                info["DirectoryId"],
                json!(id),
                "case {:?}: block {block} is reported for the directory it was added to, \
                 got {info}",
                case.name
            );
            // The block is not in service the instant the add answers: the first value
            // of the modelled `IpRouteStatusMsg` enum is what it carries until the
            // clock-owed transition falls due.
            assert_eq!(
                info["IpRouteStatusMsg"],
                json!("Adding"),
                "case {:?}: block {block} starts in Adding, got {info}",
                case.name
            );
            assert_eq!(
                info["Description"],
                route["Description"],
                "case {:?}: block {block} reports the description it was added with, \
                 got {info}",
                case.name
            );
            assert!(
                info["AddedDateTime"].is_number(),
                "case {:?}: block {block} reports when it was added, got {info}",
                case.name
            );
            // Each family is reported in its own member, and only in its own member.
            let expect_v4 = route.get("CidrIp").is_some();
            assert_eq!(
                info["CidrIp"].is_string(),
                expect_v4,
                "case {:?}: block {block} reports CidrIp only for an IPv4 block, got {info}",
                case.name
            );
            assert_eq!(
                info["CidrIpv6"].is_string(),
                !expect_v4,
                "case {:?}: block {block} reports CidrIpv6 only for an IPv6 block, got {info}",
                case.name
            );
        }

    }

    // The status transition each add owes falls due on the clock, not on a later call:
    // nothing but time passes here.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, id) in cases.iter().zip(&ids) {
        let advanced = routes_of(&router, id, case.name).await;
        for block in case.blocks {
            assert_eq!(
                advanced[*block]["IpRouteStatusMsg"],
                json!("Added"),
                "case {:?}: block {block} reaches Added once the clock says so, got {:?}",
                case.name,
                advanced[*block]
            );
        }
    }
}

#[tokio::test]
async fn a_block_the_directory_already_routes_is_refused_and_changes_nothing() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "duplicates").await;
    ok(
        &router,
        "AddIpRoutes",
        json!({
            "DirectoryId": id,
            "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }, { "CidrIpv6": "2001:db8::/32" }]
        }),
        "duplicates",
    )
    .await;
    let before = routes_of(&router, &id, "duplicates").await;

    // The rows differ in HOW the block repeats: against the store, twice within one
    // request, and alongside a block that would otherwise be new.
    let cases: [(&str, Value); 4] = [
        (
            "an IPv4 block already stored",
            json!([{ "CidrIp": "198.51.100.0/24" }]),
        ),
        (
            "an IPv6 block already stored",
            json!([{ "CidrIpv6": "2001:db8::/32" }]),
        ),
        (
            "a block named twice in one request",
            json!([{ "CidrIp": "203.0.113.0/24" }, { "CidrIp": "203.0.113.0/24" }]),
        ),
        (
            "a new block beside one already stored",
            json!([{ "CidrIp": "192.0.2.0/24" }, { "CidrIp": "198.51.100.0/24" }]),
        ),
    ];

    for (case, routes) in cases {
        refused(
            &router,
            "AddIpRoutes",
            json!({ "DirectoryId": id, "IpRoutes": routes }),
            "ClientException",
            case,
        )
        .await;
        let after = routes_of(&router, &id, case).await;
        let before_blocks: BTreeSet<&String> = before.keys().collect();
        let after_blocks: BTreeSet<&String> = after.keys().collect();
        assert_eq!(
            after_blocks, before_blocks,
            "case {case:?}: a refused add leaves the stored blocks untouched, \
             got {after_blocks:?}"
        );
    }
}

#[tokio::test]
async fn the_published_block_limit_is_the_bound_on_what_a_directory_routes() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "the block limit").await;

    // The bound comes from `IpRouteLimitExceededException`'s own documentation: "The
    // maximum allowed number of IP addresses was exceeded. The default limit is 100 IP
    // address blocks." Exactly the limit is accepted; one past it is refused.
    let blocks: Vec<String> = (0..MAX_IP_ROUTES).map(|i| format!("10.{i}.0.0/16")).collect();
    let at_limit: Vec<Value> = blocks
        .iter()
        .map(|block| json!({ "CidrIp": block }))
        .collect();
    ok(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": at_limit }),
        "exactly the limit",
    )
    .await;
    let stored = routes_of(&router, &id, "exactly the limit").await;
    assert_eq!(
        stored.len(),
        MAX_IP_ROUTES,
        "case {:?}: all {MAX_IP_ROUTES} blocks are stored, got {}",
        "exactly the limit",
        stored.len()
    );

    refused(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "10.200.0.0/16" }] }),
        "IpRouteLimitExceededException",
        "one block past the limit",
    )
    .await;
    let after = routes_of(&router, &id, "one block past the limit").await;
    assert_eq!(
        after.len(),
        MAX_IP_ROUTES,
        "case {:?}: a refused add stores nothing, got {}",
        "one block past the limit",
        after.len()
    );

    // A single request longer than the limit cannot succeed whatever is stored, so it
    // is refused against a directory that routes nothing at all.
    let empty = active_directory(&router, "sales.example.com", "one request past the limit").await;
    let past_limit: Vec<Value> = (0..=MAX_IP_ROUTES)
        .map(|i| json!({ "CidrIp": format!("10.{i}.0.0/16") }))
        .collect();
    refused(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": empty, "IpRoutes": past_limit }),
        "IpRouteLimitExceededException",
        "one request past the limit",
    )
    .await;
    let nothing = routes_of(&router, &empty, "one request past the limit").await;
    assert!(
        nothing.is_empty(),
        "case {:?}: the refused request stored nothing, got {nothing:?}",
        "one request past the limit"
    );
}

#[tokio::test]
async fn a_route_set_that_names_no_single_block_is_refused() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "corp.example.com", "malformed route sets").await;

    // Both members of `IpRoute` are optional in the model, so a route that names
    // neither block, or both, identifies no single block for the directory to route.
    let cases: [(&str, Value); 3] = [
        ("no blocks at all", json!([])),
        ("a route naming neither member", json!([{ "Description": "nothing" }])),
        (
            "a route naming both members",
            json!([{ "CidrIp": "198.51.100.0/24", "CidrIpv6": "2001:db8::/32" }]),
        ),
    ];
    for (case, routes) in cases {
        refused(
            &router,
            "AddIpRoutes",
            json!({ "DirectoryId": id, "IpRoutes": routes }),
            "InvalidParameterException",
            case,
        )
        .await;
        let stored = routes_of(&router, &id, case).await;
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused request stores nothing, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn routing_a_block_needs_a_directory_that_is_in_service() {
    let (router, _engine) = emulator();
    // The create answers while the directory is still in the first stage of the
    // modelled `DirectoryStage` enum, so a caller acting immediately after it sees the
    // transient state and the modelled refusal that goes with it.
    let id = create_directory(
        &router,
        minimal_create("corp.example.com"),
        "a directory still being built",
    )
    .await;
    refused(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }] }),
        "DirectoryUnavailableException",
        "a directory still being built",
    )
    .await;

    // Once the transition the create owes has fallen due, the same request is accepted.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "AddIpRoutes",
        json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }] }),
        "a directory in service",
    )
    .await;
    let stored = routes_of(&router, &id, "a directory in service").await;
    assert!(
        stored.contains_key("198.51.100.0/24"),
        "case {:?}: the accepted add is stored, got {stored:?}",
        "a directory in service"
    );
}

#[tokio::test]
async fn an_operation_that_names_an_absent_directory_is_refused_before_anything_else() {
    let (router, _engine) = emulator();

    // Every row is a well-formed id no directory is stored under, and every operation
    // below resolves the directory before it does anything with the rest of the
    // request — which is why the refusal does not depend on what else the request says.
    for (id, class) in ABSENT_DIRECTORY_IDS {
        let entity_shaped: [(&str, Value); 4] = [
            (
                "AddIpRoutes",
                json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }] }),
            ),
            ("ListIpRoutes", json!({ "DirectoryId": id })),
            (
                "RemoveTagsFromResource",
                json!({ "ResourceId": id, "TagKeys": ["env"] }),
            ),
            ("GetSnapshotLimits", json!({ "DirectoryId": id })),
        ];
        for (operation, body) in entity_shaped {
            refused(
                &router,
                operation,
                body,
                "EntityDoesNotExistException",
                &format!("{operation} on {class} {id}"),
            )
            .await;
        }

        // The two settings reads model `DirectoryDoesNotExistException` instead, and
        // the enum member a caller may pass is validated BEFORE the directory is
        // resolved, so a request carrying a valid wire value still fails resolution.
        let directory_shaped: [(&str, Value); 4] = [
            ("DescribeLDAPSSettings", json!({ "DirectoryId": id })),
            (
                "DescribeLDAPSSettings",
                json!({ "DirectoryId": id, "Type": LDAPS_TYPE }),
            ),
            (
                "DescribeClientAuthenticationSettings",
                json!({ "DirectoryId": id }),
            ),
            (
                "DescribeClientAuthenticationSettings",
                json!({ "DirectoryId": id, "Type": "SmartCard", "Limit": 50 }),
            ),
        ];
        for (operation, body) in directory_shaped {
            refused(
                &router,
                operation,
                body,
                "DirectoryDoesNotExistException",
                &format!("{operation} on {class} {id}"),
            )
            .await;
        }

        // `RemoveRegion` is refused the same way, whatever Region it is signed for.
        for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
            refused_in(
                &router,
                region,
                "RemoveRegion",
                json!({ "DirectoryId": id }),
                "DirectoryDoesNotExistException",
                &format!("RemoveRegion in {region} on {class} {id}"),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn an_input_the_model_refuses_is_refused_before_the_directory_is_resolved() {
    // The ordering a client depends on: the modelled constraints are the boundary
    // validator's job and run BEFORE the handler, so a request that breaks one is
    // refused on the input — never with the not-found error that a well-formed request
    // for the same absent directory would get.
    let (router, _engine) = emulator();
    let absent = ABSENT_DIRECTORY_IDS[0].0;
    let cases: [(&str, &str, Value); 5] = [
        (
            "a Type outside the LDAPSType enum",
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": absent, "Type": "Server" }),
        ),
        (
            "a Limit below the modelled range",
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": absent, "Limit": 0 }),
        ),
        (
            "a Limit above the modelled range",
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": absent, "Limit": 51 }),
        ),
        (
            "a Type outside the ClientAuthenticationType enum",
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": absent, "Type": "Password" }),
        ),
        (
            "a Limit above the modelled range",
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": absent, "Limit": 51 }),
        ),
    ];
    for (case, operation, body) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} refuses the input, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
        assert!(
            !wire_type.contains("DirectoryDoesNotExistException"),
            "case {case:?}: {operation} must refuse the input rather than resolve the \
             directory, got {parsed}"
        );
        assert!(
            !wire_type.contains("NotImplemented"),
            "case {case:?}: {operation} must not answer the scaffold signal, got {parsed}"
        );
    }

    // `AddIpRoutes` orders its own input checks the same way, deliberately: a route set
    // no directory could ever accept is refused on the input, so the answer does not
    // depend on whether the id names a stored directory. The rows are the three such
    // sets, each sent for an absent id and for a stored one.
    let stored = active_directory(&router, "corp.example.com", "route-set ordering").await;
    let past_limit: Vec<Value> = (0..=MAX_IP_ROUTES)
        .map(|i| json!({ "CidrIp": format!("10.{i}.0.0/16") }))
        .collect();
    let route_sets: [(&str, Value, &str); 3] = [
        ("an empty route set", json!([]), "InvalidParameterException"),
        (
            "a route naming no single block",
            json!([{ "Description": "nothing" }]),
            "InvalidParameterException",
        ),
        (
            "more blocks than any directory may hold",
            Value::Array(past_limit),
            "IpRouteLimitExceededException",
        ),
    ];
    for (case, routes, expected_code) in route_sets {
        for (directory_id, class) in [(absent, "an absent directory"), (stored.as_str(), "a stored directory")] {
            refused(
                &router,
                "AddIpRoutes",
                json!({ "DirectoryId": directory_id, "IpRoutes": routes }),
                expected_code,
                &format!("{case} for {class}"),
            )
            .await;
        }
    }

    // The same requests, with every modelled constraint satisfied, DO reach resolution.
    for (case, operation, body) in [
        (
            "the one LDAPSType value, at each end of the Limit range",
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": absent, "Type": LDAPS_TYPE, "Limit": 1 }),
        ),
        (
            "a ClientAuthenticationType value with the largest Limit",
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": absent, "Type": "SmartCardOrPassword", "Limit": 50 }),
        ),
    ] {
        refused(
            &router,
            operation,
            body,
            "DirectoryDoesNotExistException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn removing_tag_keys_leaves_the_stored_map_without_them() {
    let (router, _engine) = emulator();

    // The rows differ in how many keys are removed and whether they are keys the
    // directory carries: one, several, all of them, keys it never had, and none at all.
    let cases: [(&str, Vec<&str>, Vec<&str>); 5] = [
        ("one key of three", vec!["env"], vec!["owner", "tier"]),
        (
            "two keys of three",
            vec!["env", "tier"],
            vec!["owner"],
        ),
        (
            "every key",
            vec!["env", "owner", "tier"],
            vec![],
        ),
        (
            "a key the directory never had",
            vec!["absent"],
            vec!["env", "owner", "tier"],
        ),
        ("no keys at all", vec![], vec!["env", "owner", "tier"]),
    ];

    for (case, remove, expected) in cases {
        let id = create_directory(&router, minimal_create("corp.example.com"), case).await;
        ok(
            &router,
            "AddTagsToResource",
            json!({
                "ResourceId": id,
                "Tags": [
                    { "Key": "env", "Value": "dev" },
                    { "Key": "owner", "Value": "team-a" },
                    { "Key": "tier", "Value": "gold" }
                ]
            }),
            case,
        )
        .await;

        ok(
            &router,
            "RemoveTagsFromResource",
            json!({ "ResourceId": id, "TagKeys": remove }),
            case,
        )
        .await;

        let listed = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceId": id }),
            case,
        )
        .await;
        let remaining = tag_map(&listed, case);
        let found: Vec<&str> = remaining.keys().map(String::as_str).collect();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the keys {expected:?} to remain, got {found:?}"
        );
        for key in &expected {
            let value = match *key {
                "env" => "dev",
                "owner" => "team-a",
                _ => "gold",
            };
            assert_eq!(
                remaining.get(*key).map(String::as_str),
                Some(value),
                "case {case:?}: a key that stayed keeps its value, got {remaining:?}"
            );
        }

        // Removing the same keys again is not an error, and leaves the same map: the
        // outcome of the request is the keys' absence either way.
        ok(
            &router,
            "RemoveTagsFromResource",
            json!({ "ResourceId": id, "TagKeys": remove }),
            case,
        )
        .await;
        let again = tag_map(
            &ok(
                &router,
                "ListTagsForResource",
                json!({ "ResourceId": id }),
                case,
            )
            .await,
            case,
        );
        assert_eq!(
            again, remaining,
            "case {case:?}: removing the same keys twice changes nothing, got {again:?}"
        );
    }
}

#[tokio::test]
async fn snapshot_limits_are_the_quota_and_the_count_the_store_holds() {
    let (router, _engine) = emulator();
    let id = create_directory(&router, minimal_create("corp.example.com"), "snapshot limits").await;
    // A second directory, so the count reported for one is shown to be that directory's
    // own rather than the store's total.
    let other = create_directory(
        &router,
        minimal_create("other.example.com"),
        "snapshot limits",
    )
    .await;

    // A directory nobody has snapshotted reports the quota and a count of zero.
    let answer = ok(
        &router,
        "GetSnapshotLimits",
        json!({ "DirectoryId": id }),
        "a directory with no snapshots",
    )
    .await;
    assert_eq!(
        answer["SnapshotLimits"],
        json!({
            "ManualSnapshotsLimit": MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY,
            "ManualSnapshotsCurrentCount": 0,
            "ManualSnapshotsLimitReached": false
        }),
        "case {:?}: expected the quota and an empty count, got {answer}",
        "a directory with no snapshots"
    );

    // `ManualSnapshotsLimitReached` ("Indicates if the manual snapshot limit has been
    // reached") is derived from the quota and the count, so the rows walk the count up
    // through the published limit of 5 by taking snapshots over HTTP — one per row.
    for taken in 1..=MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY {
        let case = format!("{taken} manual snapshots");
        ok(
            &router,
            "CreateSnapshot",
            json!({ "DirectoryId": id, "Name": format!("nightly-{taken}") }),
            &case,
        )
        .await;
        let answer = ok(
            &router,
            "GetSnapshotLimits",
            json!({ "DirectoryId": id }),
            &case,
        )
        .await;
        assert_eq!(
            answer["SnapshotLimits"],
            json!({
                "ManualSnapshotsLimit": MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY,
                "ManualSnapshotsCurrentCount": taken,
                "ManualSnapshotsLimitReached": taken >= MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY
            }),
            "case {case:?}: expected the limit reached only at the quota, got {answer}"
        );

        // The other directory's limits are untouched by all of it: the count is read
        // per directory, not per store.
        let untouched = ok(
            &router,
            "GetSnapshotLimits",
            json!({ "DirectoryId": other }),
            &case,
        )
        .await;
        assert_eq!(
            untouched["SnapshotLimits"]["ManualSnapshotsCurrentCount"],
            json!(0),
            "case {case:?}: another directory still holds no snapshots, got {untouched}"
        );
    }

    // One past the quota is refused by `CreateSnapshot`, so the reported count cannot
    // exceed the limit it is compared against.
    refused(
        &router,
        "CreateSnapshot",
        json!({ "DirectoryId": id, "Name": "one-too-many" }),
        "SnapshotLimitExceededException",
        "one snapshot past the quota",
    )
    .await;
    let answer = ok(
        &router,
        "GetSnapshotLimits",
        json!({ "DirectoryId": id }),
        "one snapshot past the quota",
    )
    .await;
    assert_eq!(
        answer["SnapshotLimits"]["ManualSnapshotsCurrentCount"],
        json!(MAX_MANUAL_SNAPSHOTS_PER_DIRECTORY),
        "case {:?}: the refused create left the count at the quota, got {answer}",
        "one snapshot past the quota"
    );
}

#[tokio::test]
async fn ldaps_settings_report_exactly_what_the_store_holds() {
    let (router, engine) = emulator();

    // A directory nothing has enabled LDAPS for holds no setting, so the read reports
    // an empty list rather than an invented status.
    let bare = create_directory(&router, minimal_create("bare.example.com"), "no LDAPS").await;
    for body in [
        json!({ "DirectoryId": bare }),
        json!({ "DirectoryId": bare, "Type": LDAPS_TYPE }),
        json!({ "DirectoryId": bare, "Limit": 1 }),
    ] {
        let answer = ok(&router, "DescribeLDAPSSettings", body.clone(), "no LDAPS").await;
        assert_eq!(
            answer["LDAPSSettingsInfo"],
            json!([]),
            "case {:?}: {body} reports no settings, got {answer}",
            "no LDAPS"
        );
        assert!(
            answer["NextToken"].is_null(),
            "case {:?}: a complete page carries no NextToken, got {answer}",
            "no LDAPS"
        );
    }

    // One row per value of the modelled `LDAPSStatus` catalog: whatever the store
    // holds for the directory's one `LDAPSType` is what is reported, member for member.
    for (row, status) in LDAPS_STATUSES.iter().enumerate() {
        let case = format!("LDAPS status {status}");
        let id = create_directory(
            &router,
            minimal_create(&format!("ldaps-{row}.example.com")),
            &case,
        )
        .await;
        let reason = format!("set to {status} by the test");
        let updated = UNIX_EPOCH + SEEDED_INSTANT + Duration::from_secs(row as u64);
        with_directory(&engine, &id, |directory| {
            directory.ldaps_settings.insert(
                LDAPS_TYPE.to_string(),
                LdapsSetting {
                    status: (*status).to_string(),
                    status_reason: Some(reason.clone()),
                    last_updated: updated,
                    // A seeded status owes the clock nothing: each row pins what the
                    // read reports for the status it holds, so nothing may move it.
                    status_due_at: None,
                },
            );
        });

        let answer = ok(
            &router,
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": id, "Type": LDAPS_TYPE }),
            &case,
        )
        .await;
        let reported = answer["LDAPSSettingsInfo"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected LDAPSSettingsInfo, got {answer}"));
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the one stored setting is reported, got {answer}"
        );
        assert_eq!(
            reported[0]["LDAPSStatus"],
            json!(status),
            "case {case:?}: the stored status is reported, got {answer}"
        );
        assert_eq!(
            reported[0]["LDAPSStatusReason"],
            json!(reason),
            "case {case:?}: the stored reason is reported, got {answer}"
        );
        assert_eq!(
            reported[0]["LastUpdatedDateTime"].as_f64(),
            Some(epoch_seconds(updated)),
            "case {case:?}: the stored instant is reported, got {answer}"
        );

        // The unfiltered read reports the same setting: the only value the modelled
        // `LDAPSType` enum carries is the one the store is keyed by.
        let unfiltered = ok(
            &router,
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": id }),
            &case,
        )
        .await;
        assert_eq!(
            unfiltered["LDAPSSettingsInfo"], answer["LDAPSSettingsInfo"],
            "case {case:?}: an unfiltered read reports the stored setting too, got {unfiltered}"
        );
    }
}

#[tokio::test]
async fn client_authentication_settings_are_filtered_and_paged_from_the_store() {
    let (router, engine) = emulator();

    let bare = create_directory(
        &router,
        minimal_create("bare.example.com"),
        "no client authentication",
    )
    .await;
    let answer = ok(
        &router,
        "DescribeClientAuthenticationSettings",
        json!({ "DirectoryId": bare }),
        "no client authentication",
    )
    .await;
    assert_eq!(
        answer["ClientAuthenticationSettingsInfo"],
        json!([]),
        "case {:?}: a directory with nothing configured reports an empty list, got {answer}",
        "no client authentication"
    );

    // Both values of the modelled `ClientAuthenticationType` catalog are recorded, each
    // with a different `ClientAuthenticationStatus`, so a filtered read cannot pass by
    // reporting the other one.
    let id = create_directory(
        &router,
        minimal_create("smartcard.example.com"),
        "both authentication types",
    )
    .await;
    let seeded: Vec<(String, String, SystemTime)> = CLIENT_AUTHENTICATION_TYPES
        .iter()
        .zip(CLIENT_AUTHENTICATION_STATUSES)
        .enumerate()
        .map(|(row, (authentication_type, status))| {
            (
                (*authentication_type).to_string(),
                status.to_string(),
                UNIX_EPOCH + SEEDED_INSTANT + Duration::from_secs(row as u64),
            )
        })
        .collect();
    with_directory(&engine, &id, |directory| {
        for (authentication_type, status, updated) in &seeded {
            directory.client_authentication_settings.insert(
                authentication_type.clone(),
                ClientAuthenticationSetting {
                    status: status.clone(),
                    last_updated: *updated,
                },
            );
        }
    });

    // Unfiltered: every recorded type, each reporting its own status and instant.
    let case = "every authentication type";
    let answer = ok(
        &router,
        "DescribeClientAuthenticationSettings",
        json!({ "DirectoryId": id }),
        case,
    )
    .await;
    let reported = answer["ClientAuthenticationSettingsInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected the settings list, got {answer}"))
        .clone();
    assert_eq!(
        reported.len(),
        seeded.len(),
        "case {case:?}: every recorded type is reported, got {answer}"
    );
    for (authentication_type, status, updated) in &seeded {
        let info = reported
            .iter()
            .find(|info| info["Type"] == json!(authentication_type))
            .unwrap_or_else(|| panic!("case {case:?}: {authentication_type} is reported, got {answer}"));
        assert_eq!(
            info["Status"],
            json!(status),
            "case {case:?}: {authentication_type} reports its stored status, got {info}"
        );
        assert_eq!(
            info["LastUpdatedDateTime"].as_f64(),
            Some(epoch_seconds(*updated)),
            "case {case:?}: {authentication_type} reports its stored instant, got {info}"
        );
    }

    // Filtered: one row per type, and only that type comes back.
    for (authentication_type, status, _) in &seeded {
        let case = format!("filtered to {authentication_type}");
        let answer = ok(
            &router,
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": id, "Type": authentication_type }),
            &case,
        )
        .await;
        assert_eq!(
            answer["ClientAuthenticationSettingsInfo"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {case:?}: exactly the requested type is reported, got {answer}"
        );
        let info = &answer["ClientAuthenticationSettingsInfo"][0];
        assert_eq!(
            (info["Type"].clone(), info["Status"].clone()),
            (json!(authentication_type), json!(status)),
            "case {case:?}: the requested type reports its stored status, got {answer}"
        );
    }

    // Paged: a page smaller than the record count hands back a token, and the token
    // resumes after the type the page ended on.
    let case = "one type per page";
    let first = ok(
        &router,
        "DescribeClientAuthenticationSettings",
        json!({ "DirectoryId": id, "Limit": 1 }),
        case,
    )
    .await;
    assert_eq!(
        first["ClientAuthenticationSettingsInfo"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "case {case:?}: a page of one carries one setting, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page carries a NextToken, got {first}"))
        .to_string();
    let second = ok(
        &router,
        "DescribeClientAuthenticationSettings",
        json!({ "DirectoryId": id, "Limit": 1, "NextToken": token }),
        case,
    )
    .await;
    assert!(
        second["NextToken"].is_null(),
        "case {case:?}: the final page carries no NextToken, got {second}"
    );
    let paged: Vec<Value> = first["ClientAuthenticationSettingsInfo"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .chain(
            second["ClientAuthenticationSettingsInfo"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        )
        .collect();
    assert_eq!(
        paged, reported,
        "case {case:?}: the pages together are the unpaginated listing, got {paged:?}"
    );

    // A token this listing did not issue is refused with the error each operation
    // models for it: `InvalidParameterException` here (the operation models no
    // `InvalidNextTokenException`), `InvalidNextTokenException` for the two listings
    // that do model it. The rows differ in HOW the token is not ours: not hex at all,
    // and hex that decodes to something this service never minted.
    for (case, bad_token) in [
        ("a token that is not hex", "not-a-token"),
        ("hex that is not a cursor this service issued", "6162636465"),
    ] {
        refused(
            &router,
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": id, "NextToken": bad_token }),
            "InvalidParameterException",
            case,
        )
        .await;
        refused(
            &router,
            "DescribeLDAPSSettings",
            json!({ "DirectoryId": id, "NextToken": bad_token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
        refused(
            &router,
            "ListIpRoutes",
            json!({ "DirectoryId": id, "NextToken": bad_token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }

    // And a token one listing DID issue is refused by another: every cursor is tagged
    // with the listing that minted it, so it cannot be replayed across operations.
    let case = "a token another listing issued";
    for operation in ["DescribeLDAPSSettings", "ListIpRoutes"] {
        refused(
            &router,
            operation,
            json!({ "DirectoryId": id, "NextToken": token }),
            "InvalidNextTokenException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn remove_region_acts_on_the_region_the_call_was_signed_for() {
    let (router, engine) = emulator();

    // The rows differ in which Region the directory was created in, so what counts as
    // its primary Region is read from the store rather than fixed.
    for primary in ["us-east-1", "eu-west-1", "ap-northeast-1"] {
        let case = format!("a directory created in {primary}");
        let id = active_directory_in(&router, primary, "corp.example.com", &case).await;

        // Its own Region is the primary one, which this operation does not remove:
        // "You cannot remove the primary Region with this operation. Instead, use the
        // DeleteDirectory API."
        refused_in(
            &router,
            primary,
            "RemoveRegion",
            json!({ "DirectoryId": id }),
            "UnsupportedOperationException",
            &case,
        )
        .await;

        // A Region the directory does not exist in is not found there, whatever the id
        // names elsewhere.
        for elsewhere in ["us-west-2", "sa-east-1"] {
            refused_in(
                &router,
                elsewhere,
                "RemoveRegion",
                json!({ "DirectoryId": id }),
                "DirectoryDoesNotExistException",
                &format!("{case}, called in {elsewhere}"),
            )
            .await;
        }

        // A Region the directory IS replicated to is removed, and is then a Region it
        // no longer exists in. A Simple AD directory does not replicate across Regions,
        // so the replication is seeded directly.
        let replica = "us-west-2";
        with_directory(&engine, &id, |directory| {
            directory
                .additional_regions
                .insert(replica.to_string(), settled_replication(replica, SystemTime::now()));
        });
        let (status, body) =
            call_in_region(&router, "RemoveRegion", json!({ "DirectoryId": id }), replica).await;
        assert_eq!(
            status, 200,
            "case {case:?}: removing the replicated Region {replica} answers 200, \
             got {status} with {body}"
        );
        let left = with_directory(&engine, &id, |directory| {
            directory.additional_regions.clone()
        });
        assert!(
            left.is_empty(),
            "case {case:?}: the replicated Region is gone from the store, got {left:?}"
        );
        refused_in(
            &router,
            replica,
            "RemoveRegion",
            json!({ "DirectoryId": id }),
            "DirectoryDoesNotExistException",
            &format!("{case}, {replica} removed twice"),
        )
        .await;

        // The primary Region is untouched by all of this: the directory is still there.
        let described = ok(
            &router,
            "DescribeDirectories",
            json!({ "DirectoryIds": [id] }),
            &case,
        )
        .await;
        assert_eq!(
            described["DirectoryDescriptions"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {case:?}: the directory itself survives, got {described}"
        );
    }
}

#[tokio::test]
async fn removing_a_region_needs_a_directory_that_is_in_service() {
    let (router, engine) = emulator();
    let id = create_directory(
        &router,
        minimal_create("corp.example.com"),
        "a directory still being built",
    )
    .await;
    with_directory(&engine, &id, |directory| {
        directory.additional_regions.insert(
            "us-west-2".to_string(),
            settled_replication("us-west-2", SystemTime::now()),
        );
    });
    refused_in(
        &router,
        "us-west-2",
        "RemoveRegion",
        json!({ "DirectoryId": id }),
        "DirectoryUnavailableException",
        "a directory still being built",
    )
    .await;

    tokio::time::sleep(PAST_ONE_SPAN).await;
    let (status, body) =
        call_in_region(&router, "RemoveRegion", json!({ "DirectoryId": id }), "us-west-2").await;
    assert_eq!(
        status, 200,
        "case {:?}: the same call succeeds once the directory is in service, \
         got {status} with {body}",
        "a directory in service"
    );
}

#[tokio::test]
async fn none_of_these_operations_answers_the_scaffold_501() {
    // The census this change is against: each of these operations answered HTTP 501
    // `NotImplemented` for every request. None of them may answer it again — not for a
    // request that succeeds, and not for one that is refused.
    let (router, _engine) = emulator();
    let id = create_directory(&router, minimal_create("corp.example.com"), "no 501").await;
    let calls: [(&str, Value); 7] = [
        (
            "AddIpRoutes",
            json!({ "DirectoryId": id, "IpRoutes": [{ "CidrIp": "198.51.100.0/24" }] }),
        ),
        ("ListIpRoutes", json!({ "DirectoryId": id })),
        (
            "RemoveTagsFromResource",
            json!({ "ResourceId": id, "TagKeys": ["env"] }),
        ),
        ("GetSnapshotLimits", json!({ "DirectoryId": id })),
        ("DescribeLDAPSSettings", json!({ "DirectoryId": id })),
        (
            "DescribeClientAuthenticationSettings",
            json!({ "DirectoryId": id }),
        ),
        ("RemoveRegion", json!({ "DirectoryId": id })),
    ];
    for (operation, body) in calls {
        let (status, parsed) = call(&router, operation, body).await;
        assert_ne!(
            status, 501,
            "case {operation:?}: no longer a scaffold stub, got {status} with {parsed}"
        );
        assert!(
            !parsed.to_string().contains("NotImplemented"),
            "case {operation:?}: the answer must not carry the scaffold signal, got {parsed}"
        );
    }
}

