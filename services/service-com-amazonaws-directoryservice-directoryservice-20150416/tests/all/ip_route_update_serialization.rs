// SPDX-License-Identifier: CC0-1.0

//! A directory takes ONE change to its IP routing at a time, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router. Nothing here reaches into the state struct, so
//! an answer that is not computed from persisted state cannot pass.
//!
//! The RULE under test, asserted over a table rather than one recorded call: an accepted
//! `AddIpRoutes` puts the directory's routing under an update that the clock settles one
//! transition span later, and a further `AddIpRoutes` arriving while that update is still
//! in flight is refused — 400 `ClientException`, stating that an update is already in
//! progress for the directory the request named — WITHOUT being compared to the routes the
//! in-flight change has written and without writing anything itself. Once the update has
//! settled the same request is evaluated against the stored routes again: a block the
//! directory now routes is the already-routed refusal, and a block it does not is accepted.
//!
//! The rows differ along what the second request names (the same block, a block nothing
//! routes, an IPv6 block, several blocks, a new block beside the in-flight one), along
//! which change is in flight (an add's `Adding`, a removal's `Removing`), and — in the
//! last case — along nothing at all: two genuinely overlapping adds of ONE block, which
//! must settle as exactly one accepted change, one refusal, and one stored route.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{call, ok, PAST_ONE_SPAN};
use crate::directory_networking_and_settings::{active_directory, emulator, routes_of};

/// The block each directory under test is given before anything else happens: the one the
/// in-flight change is made of.
const SEEDED: &str = "198.51.100.0/24";

/// The `IpRouteStatusMsg` a block that is in service carries: the `Added` value of the
/// modelled enum (`smithy_query(query="shape", shape="IpRouteStatusMsg")`).
const STATUS_ADDED: &str = "Added";

/// An `AddIpRoutes` body naming `routes` for `directory_id`.
fn add_body(directory_id: &str, routes: Value) -> Value {
    json!({ "DirectoryId": directory_id, "IpRoutes": routes })
}

/// The whole `Message` of a 400 refusal of `expected_code`, with its request-id suffix
/// stripped: the diagnostic the service worded, which is what the templates here are
/// asserted against.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> String {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
    stated(&parsed, case)
}

/// The `Message` of a parsed refusal, without the request-id suffix every refusal of this
/// service closes with.
fn stated(parsed: &Value, case: &str) -> String {
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"));
    message
        .rsplit_once(" : RequestId: ")
        .map(|(text, _)| text)
        .unwrap_or(message)
        .to_string()
}

/// The sentence Directory Service states when a directory is already being changed.
fn update_in_progress(directory_id: &str) -> String {
    format!(
        "An update is already in progress for directory {directory_id}. Please wait until \
         this update is complete before making another change."
    )
}

/// The sentence Directory Service states for blocks a directory already routes.
fn already_routed(blocks: &[&str]) -> String {
    format!(
        "The IP route(s) [{blocks}] already exist. If you want to recreate them, please \
         delete the existing routes first.",
        blocks = blocks.join(", ")
    )
}

/// One row of the table: what the second request names, and — once the in-flight change
/// has settled — whether that same request is accepted or refused as already routed.
struct Row {
    case: &'static str,
    /// The `IpRoutes` the second request carries.
    routes: fn() -> Value,
    /// The blocks the settled retry must report as already routed; empty when the retry is
    /// accepted instead.
    already_routed_after: &'static [&'static str],
    /// The blocks the directory must route once everything has settled.
    settled_blocks: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the same block the in-flight change is adding",
            routes: || json!([{ "CidrIp": SEEDED }]),
            already_routed_after: &[SEEDED],
            settled_blocks: &[SEEDED],
        },
        Row {
            case: "an IPv4 block nothing routes yet",
            routes: || json!([{ "CidrIp": "203.0.113.0/24" }]),
            already_routed_after: &[],
            settled_blocks: &[SEEDED, "203.0.113.0/24"],
        },
        Row {
            case: "an IPv6 block",
            routes: || json!([{ "CidrIpv6": "2001:db8::/32" }]),
            already_routed_after: &[],
            settled_blocks: &[SEEDED, "2001:db8::/32"],
        },
        Row {
            case: "several new blocks at once",
            routes: || {
                json!([
                    { "CidrIp": "192.0.2.0/25", "Description": "office" },
                    { "CidrIpv6": "fd00::/8" },
                ])
            },
            already_routed_after: &[],
            settled_blocks: &[SEEDED, "192.0.2.0/25", "fd00::/8"],
        },
        Row {
            case: "a new block beside the one being added",
            routes: || json!([{ "CidrIp": "203.0.113.7/32" }, { "CidrIp": SEEDED }]),
            already_routed_after: &[SEEDED],
            settled_blocks: &[SEEDED],
        },
    ]
}

#[tokio::test]
async fn an_add_inside_the_update_window_is_refused_as_an_update_in_progress() {
    let (router, _engine) = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        // Each row gets its own directory, so the only change in flight is the one this
        // row seeds.
        let id = active_directory(&router, &format!("routes-{index}.example.com"), case).await;
        ok(&router, "AddIpRoutes", add_body(&id, json!([{ "CidrIp": SEEDED }])), case).await;

        // Inside the window the second request is refused for the DIRECTORY's sake: the
        // sentence names the directory the request asked to change, and nothing about the
        // blocks it named.
        let message = refusal(
            &router,
            "AddIpRoutes",
            add_body(&id, (row.routes)()),
            "ClientException",
            case,
        )
        .await;
        assert_eq!(
            message,
            update_in_progress(&id),
            "case {case:?}: expected the update-in-progress sentence, got {message:?}"
        );

        // Nothing was written by the refused request: the directory still routes exactly
        // the block the in-flight change is adding.
        let during = routes_of(&router, &id, case).await;
        let blocks: Vec<&String> = during.keys().collect();
        assert_eq!(
            blocks,
            vec![&SEEDED.to_string()],
            "case {case:?}: expected the refused add to write nothing, got {blocks:?}"
        );

        // Once the clock has settled the update, the same request is evaluated against the
        // stored routes again — which is where the already-routed refusal belongs.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        if row.already_routed_after.is_empty() {
            ok(
                &router,
                "AddIpRoutes",
                add_body(&id, (row.routes)()),
                case,
            )
            .await;
        } else {
            let message = refusal(
                &router,
                "AddIpRoutes",
                add_body(&id, (row.routes)()),
                "ClientException",
                case,
            )
            .await;
            assert_eq!(
                message,
                already_routed(row.already_routed_after),
                "case {case:?}: expected the already-routed sentence, got {message:?}"
            );
        }

        // The routing the directory settles on is what the accepted changes left.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let settled = routes_of(&router, &id, case).await;
        let found: Vec<String> = settled.keys().cloned().collect();
        let mut expected: Vec<String> = row
            .settled_blocks
            .iter()
            .map(|block| (*block).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            found, expected,
            "case {case:?}: expected the directory to route {expected:?}, got {found:?}"
        );
        for (block, info) in &settled {
            assert_eq!(
                info["IpRouteStatusMsg"],
                json!(STATUS_ADDED),
                "case {case:?}: expected block {block} to have settled, got {info}"
            );
        }
    }
}

#[tokio::test]
async fn an_add_inside_a_removal_window_is_refused_the_same_way() {
    let (router, _engine) = emulator();
    let id = active_directory(&router, "removing.example.com", "a removal in flight").await;
    ok(
        &router,
        "AddIpRoutes",
        add_body(&id, json!([{ "CidrIp": SEEDED }])),
        "a removal in flight",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A removal is the other change to a directory's routing, and it is in flight for the
    // span the clock owes it, so an add arriving inside that window is refused as the same
    // update in progress.
    ok(
        &router,
        "RemoveIpRoutes",
        json!({ "DirectoryId": &id, "CidrIps": [SEEDED] }),
        "a removal in flight",
    )
    .await;
    let message = refusal(
        &router,
        "AddIpRoutes",
        add_body(&id, json!([{ "CidrIp": "203.0.113.0/24" }])),
        "ClientException",
        "a removal in flight",
    )
    .await;
    assert_eq!(
        message,
        update_in_progress(&id),
        "case {:?}: expected the update-in-progress sentence, got {message:?}",
        "a removal in flight"
    );

    // Once the removal has fallen due the directory is free again, and the add that was
    // refused is accepted.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "AddIpRoutes",
        add_body(&id, json!([{ "CidrIp": "203.0.113.0/24" }])),
        "a removal in flight",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = routes_of(&router, &id, "a removal in flight").await;
    let found: Vec<String> = settled.keys().cloned().collect();
    assert_eq!(
        found,
        vec!["203.0.113.0/24".to_string()],
        "case {:?}: expected the removed block gone and the new one routed, got {found:?}",
        "a removal in flight"
    );
}

#[tokio::test]
async fn two_overlapping_adds_of_one_block_settle_as_one_route() {
    let (router, _engine) = emulator();
    let case = "two overlapping adds";
    let id = active_directory(&router, "overlap.example.com", case).await;

    // Both calls are issued before either has answered, so they genuinely overlap: the
    // service accepts exactly one of them and refuses the other with the condition the
    // first one put the directory in.
    let body = add_body(&id, json!([{ "CidrIp": SEEDED }]));
    let (first, second) = tokio::join!(
        call(&router, "AddIpRoutes", body.clone()),
        call(&router, "AddIpRoutes", body.clone())
    );
    let answers = [first, second];
    let accepted: Vec<&(u16, Value)> = answers.iter().filter(|(status, _)| *status == 200).collect();
    let refused: Vec<&(u16, Value)> = answers.iter().filter(|(status, _)| *status != 200).collect();
    assert_eq!(
        accepted.len(),
        1,
        "case {case:?}: expected exactly one accepted add, got {answers:?}"
    );
    assert_eq!(
        refused.len(),
        1,
        "case {case:?}: expected exactly one refused add, got {answers:?}"
    );

    let (status, parsed) = refused[0];
    assert_eq!(
        *status, 400,
        "case {case:?}: expected the loser to be refused with 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("ClientException"),
        "case {case:?}: expected ClientException on the loser, got {parsed}"
    );
    let message = stated(parsed, case);
    assert_eq!(
        message,
        update_in_progress(&id),
        "case {case:?}: expected the update-in-progress sentence, got {message:?}"
    );

    // The block the pair raced for is routed ONCE, and it settles in service.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = routes_of(&router, &id, case).await;
    let found: Vec<String> = settled.keys().cloned().collect();
    assert_eq!(
        found,
        vec![SEEDED.to_string()],
        "case {case:?}: expected exactly one route for {SEEDED}, got {found:?}"
    );
    assert_eq!(
        settled[SEEDED]["IpRouteStatusMsg"],
        json!(STATUS_ADDED),
        "case {case:?}: expected the surviving route to have settled, got {:?}",
        settled[SEEDED]
    );
}
