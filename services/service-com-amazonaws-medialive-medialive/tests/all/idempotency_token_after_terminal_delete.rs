// SPDX-License-Identifier: CC0-1.0

//! How long a caller-chosen `RequestId` (`@idempotencyToken`) stays a replay in
//! this service: a create — or a purchase — answers a retry with the resource
//! its token minted only while that resource is still one the caller may use.
//!
//! Several of this service's deletes are TOMBSTONE deletes: the record stays in
//! the account, readable in the terminal state its own enum declares
//! (`DELETED` for a reservation, an SdiSource and a multiplex, `DEREGISTERED`
//! for a node), while every mutation of it is refused from then on. A token
//! whose resource has reached that state therefore names nothing a caller can
//! use or change, so it is no longer a replay: the retry mints — or buys — a
//! resource of its own.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the reported `PurchaseOffering` case — one token, one
//! reservation, one delete — is ONE row of the first of them. Every id here is
//! one the store really minted, through the service's own creates, and the
//! request helpers are the ones the suites beside this one already have.

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::RESERVATION_RESOURCE_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_deletion::delete_multiplex;
use crate::multiplex_lifecycle::{create_body, created as created_multiplex, describe_multiplex, settings};
use crate::multiplex_programs::{create_program, describe_program, multiplex, program_settings};
use crate::node_lifecycle::{
    cluster, created as created_node, delete_node, described as described_node, node_id,
};
use crate::reservation_deletion::end_the_lease;
use crate::reservation_purchase::{describe, first_offering, purchase, RESERVATION_MEMBERS};
use crate::sdi_source_lifecycle::{created as created_source, delete_source};
use crate::wire::{call, error_code, error_message, medialive, medialive_with_engine, DEFAULT_REGION};

/// The reservation collection (`smithy_query(query="operation",
/// operation="DeleteReservation")`: `DELETE /prod/reservations/{ReservationId}`;
/// `UpdateReservation`: `PUT` on the same resource).
const RESERVATIONS: &str = "/prod/reservations";

/// A lease that ran out a while ago rather than a second ago: each row is
/// several dispatches, and a lease ending inside them would move the state
/// mid-row and fail it for timing rather than for behaviour.
const AN_HOUR: Duration = Duration::from_secs(3_600);

/// The reservation states this suite reads (`smithy_query(query="shape",
/// shape="ReservationState")`: ACTIVE | EXPIRED | CANCELED | DELETED).
const ACTIVE: &str = "ACTIVE";
const DELETED: &str = "DELETED";

/// The terminal state an SdiSource's delete leaves it in
/// (`smithy_query(query="shape", shape="SdiSourceState")`: IDLE | IN_USE |
/// DELETED) — the same word the multiplex's own enum uses
/// (`shape="MultiplexState"`), and `DEREGISTERED` is the node's
/// (`shape="NodeState"`).
const DEREGISTERED: &str = "DEREGISTERED";

/// One member of a reported resource, read as a client reads it: a member the
/// response omits and one it reports as null are the same absence.
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// One reported string member, which every id and state in this suite is.
fn text(case: &str, what: &str, body: &Value) -> String {
    member(body, what)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {what} on {body}"))
        .to_string()
}

/// A reservation in the ONE member-for-member form `Reservation` declares, so a
/// purchase's answer and a reader's can be compared whichever response carried
/// them.
fn projected(reservation: &Value) -> Value {
    Value::Object(
        RESERVATION_MEMBERS
            .iter()
            .map(|name| ((*name).to_string(), member(reservation, name)))
            .collect::<Map<String, Value>>(),
    )
}

/// `DeleteReservation` for one reservation.
async fn delete_reservation(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("{RESERVATIONS}/{id}"), None).await
}

/// `UpdateReservation` for one reservation — the mutation that tells a
/// reservation a caller may still change from a tombstone it may not.
async fn update_reservation(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("{RESERVATIONS}/{id}"), Some(body)).await
}

/// The reservation `id`, read back through `DescribeReservation`.
async fn read_back(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = describe(router, DEFAULT_REGION, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeReservation({id}): expected 200, got {status}: {body}"
    );
    projected(&body)
}

/// Every reservation id the account reports through `ListReservations`, sorted.
async fn held_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", RESERVATIONS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListReservations: expected 200, got {status}: {body}"
    );
    let mut ids: Vec<String> = body["reservations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|reservation| text(case, "reservationId", reservation))
        .collect();
    ids.sort();
    ids
}

/// One `PurchaseOffering`, asserted to have been served, as the reservation it
/// answered with.
///
/// `201 Created` is the status the model gives the operation
/// (`smithy_query(query="operation", operation="PurchaseOffering")`).
async fn purchased(
    router: &MultiServiceRouter,
    case: &str,
    offering_id: &str,
    body: Value,
) -> Value {
    let (status, answered) = purchase(router, DEFAULT_REGION, offering_id, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: PurchaseOffering({offering_id}): expected 201, got {status}: {answered}"
    );
    answered
        .get("reservation")
        .cloned()
        .filter(|reservation| !reservation.is_null())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a purchase to report a reservation, got {answered}")
        })
}

/// One purchase body: the token it carries and the choices it makes.
fn purchase_body(token: &str, count: i64, name: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("count".to_string(), json!(count));
    body.insert("requestId".to_string(), json!(token));
    if let Some(name) = name {
        body.insert("name".to_string(), json!(name));
    }
    Value::Object(body)
}

/// The id of the first offering of EVERY reservable resource type the model
/// declares (`smithy_query(query="shape", shape="ReservationResourceType")`,
/// named through the service's own constant), read off `ListOfferings` — so a
/// row that buys "another configuration" names one the catalog really holds.
async fn offering_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES {
        let case = format!("the first {resource_type} offering");
        ids.push(first_offering(router, &case, &format!("resourceType={resource_type}")).await);
    }
    ids
}

/// One row of the purchase table: how it names itself, which offering the first
/// purchase buys, whether the retry buys that same offering, and the `Count`,
/// the `Name` and the number of purchase-then-delete rounds the retry runs.
type PurchaseRow = (&'static str, usize, bool, i64, Option<&'static str>, usize);

/// Rule: a create's `@idempotencyToken` answers a retry with the resource it
/// minted only while that resource is still one a caller may use, so once the
/// reservation a `RequestId` purchased has reached the terminal `DELETED` state,
/// a purchase carrying that token BUYS a reservation of its own instead of
/// replaying the deleted one.
///
/// The rows differ along what the rule generalises over: which configuration the
/// first purchase bought (one offering per reservable resource type), whether
/// the retry is addressed to that same offering or to another, the `Count` and
/// `Name` the retry chooses, and how many purchase-then-delete rounds the one
/// token runs.
#[tokio::test]
async fn a_purchase_token_does_not_replay_a_deleted_reservation() {
    // (case, which offering the first purchase buys, whether the retry buys that
    //  same offering, the retry's `Count`, the retry's `Name`, how many rounds)
    let rows: [PurchaseRow; 5] = [
        ("the retry buys the same offering again", 0, true, 1, Some("after-the-delete"), 1),
        ("the retry buys another configuration", 1, false, 5, Some("after-the-delete"), 1),
        ("the retry names no reservation at all", 2, true, 2, None, 1),
        ("the retry buys a bulk count", 3, false, 25, Some("bulk"), 1),
        ("one token over two purchase-then-delete rounds", 0, false, 3, Some("round"), 2),
    ];

    for (case, which, same_offering, count, name, rounds) in rows {
        let (router, engine) = medialive_with_engine();
        let offerings = offering_ids(&router).await;
        let bought_from = offerings[which % offerings.len()].clone();
        let retry_from = if same_offering {
            bought_from.clone()
        } else {
            offerings[(which + 1) % offerings.len()].clone()
        };
        let token = "token-a";
        let retry = || purchase_body(token, count, name);

        let first = purchased(
            &router,
            case,
            &bought_from,
            purchase_body(token, 1, Some("before-the-delete")),
        )
        .await;
        let mut live_id = text(case, "reservationId", &first);
        let mut held = vec![live_id.clone()];

        // While the reservation is live the token IS a replay: the same purchase
        // answers with that reservation, as the store holds it, and buys nothing.
        let replayed = purchased(&router, case, &retry_from, retry()).await;
        assert_eq!(
            text(case, "reservationId", &replayed),
            live_id,
            "case {case:?}: expected a live reservation's token to replay {live_id:?}, got \
             {replayed}"
        );
        assert_eq!(
            projected(&replayed),
            read_back(&router, case, &live_id).await,
            "case {case:?}: expected the replay to report the stored reservation"
        );
        assert_eq!(
            held_ids(&router, case).await,
            held,
            "case {case:?}: expected the replay to have bought nothing"
        );

        for round in 1..=rounds {
            let case = format!("{case}, round {round}");
            // The lease runs out — the state the clock owes — and the delete
            // puts the reservation into the terminal state it then keeps.
            end_the_lease(&engine, &case, &live_id, SystemTime::now() - AN_HOUR);
            let (status, deleted) = delete_reservation(&router, &live_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteReservation({live_id}): expected 200, got {status}: \
                 {deleted}"
            );
            assert_eq!(
                member(&deleted, "state"),
                json!(DELETED),
                "case {case:?}: expected the delete to report {DELETED}, got {deleted}"
            );
            let tombstone = read_back(&router, &case, &live_id).await;

            // The window is over: the same purchase carrying the same token buys
            // a reservation of its own, against the offering the request named
            // and with the choices the request made.
            let retried = purchased(&router, &case, &retry_from, retry()).await;
            let retried_id = text(&case, "reservationId", &retried);
            assert!(
                !held.contains(&retried_id),
                "case {case:?}: expected a freshly purchased reservation, got the already held \
                 {retried_id:?}"
            );
            assert_eq!(
                member(&retried, "state"),
                json!(ACTIVE),
                "case {case:?}: expected the retry to answer with a live reservation, got \
                 {retried}"
            );
            assert_eq!(
                member(&retried, "offeringId"),
                json!(retry_from),
                "case {case:?}: expected the offering the retry named, got {retried}"
            );
            assert_eq!(
                member(&retried, "count"),
                json!(count),
                "case {case:?}: expected the count the retry asked for, got {retried}"
            );
            assert_eq!(
                member(&retried, "name"),
                name.map_or(Value::Null, |name| json!(name)),
                "case {case:?}: expected the name the retry asked for, got {retried}"
            );

            // The purchase really STORED it, and left the tombstone exactly as
            // the delete had: the account now holds both.
            assert_eq!(
                read_back(&router, &case, &retried_id).await,
                projected(&retried),
                "case {case:?}: expected the reader to report the reservation the retry bought"
            );
            assert_eq!(
                read_back(&router, &case, &live_id).await,
                tombstone,
                "case {case:?}: expected the deleted reservation to stay as it was"
            );
            held.push(retried_id.clone());
            held.sort();
            assert_eq!(
                held_ids(&router, &case).await,
                held,
                "case {case:?}: expected the account to hold the tombstone and the new \
                 reservation"
            );

            // The reservation the retry answered with is one the caller may
            // CHANGE, which the tombstone was not — and the token now replays
            // that live reservation rather than any deleted one.
            let (status, renamed) =
                update_reservation(&router, &retried_id, json!({ "name": "renamed" })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the purchased reservation to be updatable, got \
                 {status}: {renamed}"
            );
            let again = purchased(&router, &case, &retry_from, retry()).await;
            assert_eq!(
                text(&case, "reservationId", &again),
                retried_id,
                "case {case:?}: expected the token to replay the live reservation \
                 {retried_id:?}, got {again}"
            );
            assert_eq!(
                held_ids(&router, &case).await,
                held,
                "case {case:?}: expected the second replay to have bought nothing"
            );

            live_id = retried_id;
        }
    }
}

/// The resources beside a reservation whose delete keeps a TOMBSTONE record and
/// whose create carries a `RequestId` (`@idempotencyToken`) — the rest of the
/// class the rule holds over.
#[derive(Clone, Copy, Debug)]
enum Tombstoned {
    /// `CreateSdiSource` / `DeleteSdiSource`: the terminal `DELETED`, reached
    /// with the call.
    SdiSource,
    /// `CreateMultiplex` / `DeleteMultiplex`: the terminal `DELETED`, which the
    /// clock settles the accepted delete into.
    Multiplex,
    /// `CreateNode` / `DeleteNode`: the terminal `DEREGISTERED`, which the clock
    /// settles the accepted deregistration into.
    Node,
}

/// What one round of create-replay-delete-retry observed, whichever resource it
/// ran on.
struct Round {
    /// The id the first create minted.
    first_id: String,
    /// The id the token replayed while that resource was live.
    replayed_id: String,
    /// The state the resource reports once its delete has settled.
    terminal_state: String,
    /// The id the retry past the delete answered with.
    retried_id: String,
    /// The state the retry's own resource reports.
    retried_state: String,
}

/// Run one round on `kind`: a create carrying a token, a retry while the
/// resource is live, the delete taken to its terminal state, and the same create
/// carrying the same token again.
async fn a_round(kind: Tombstoned, case: &str, retried_name: &str) -> Round {
    let router = medialive();
    let token = "token-a";
    match kind {
        Tombstoned::SdiSource => {
            let body = |name: &str| json!({ "name": name, "type": "SINGLE", "requestId": token });
            let first = created_source(&router, case, body("source-a")).await;
            let first_id = text(case, "id", &first);
            let replayed = created_source(&router, case, body(retried_name)).await;
            let (status, deleted) = delete_source(&router, &first_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteSdiSource: expected 200, got {status}: {deleted}"
            );
            let retried = created_source(&router, case, body(retried_name)).await;
            Round {
                first_id,
                replayed_id: text(case, "id", &replayed),
                terminal_state: text(case, "state", &deleted),
                retried_id: text(case, "id", &retried),
                retried_state: text(case, "state", &retried),
            }
        }
        Tombstoned::Multiplex => {
            let body =
                |name: &str| create_body(name, token, settings(2_000_000, 1));
            let first = created_multiplex(&router, case, body("multiplex-a")).await;
            let first_id = text(case, "id", &first);
            let replayed = created_multiplex(&router, case, body(retried_name)).await;
            // A multiplex is deleted from the stopped state, so the create's own
            // transition is let come due first.
            wait_for_the_clock();
            let (status, deleting) = delete_multiplex(&router, &first_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
            );
            wait_for_the_clock();
            let (status, settled) = describe_multiplex(&router, &first_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeMultiplex: expected 200, got {status}: {settled}"
            );
            let retried = created_multiplex(&router, case, body(retried_name)).await;
            Round {
                first_id,
                replayed_id: text(case, "id", &replayed),
                terminal_state: text(case, "state", &settled),
                retried_id: text(case, "id", &retried),
                retried_state: text(case, "state", &retried),
            }
        }
        Tombstoned::Node => {
            let cluster_id = cluster(&router, "token-cluster").await;
            let body = |name: &str| json!({ "name": name, "requestId": token });
            let first = created_node(&router, case, &cluster_id, body("node-a")).await;
            let first_id = node_id(case, &first);
            let replayed = created_node(&router, case, &cluster_id, body(retried_name)).await;
            let (status, deleting) = delete_node(&router, &cluster_id, &first_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode: expected 202, got {status}: {deleting}"
            );
            wait_for_the_clock();
            let settled = described_node(&router, case, &cluster_id, &first_id).await;
            let retried = created_node(&router, case, &cluster_id, body(retried_name)).await;
            Round {
                first_id,
                replayed_id: node_id(case, &replayed),
                terminal_state: text(case, "state", &settled),
                retried_id: node_id(case, &retried),
                retried_state: text(case, "state", &retried),
            }
        }
    }
}

/// Rule: the same window the reservation purchase keeps is the one every other
/// tombstone-keeping resource of this service keeps — a token replays the
/// resource it minted while that resource is live, and once the delete has
/// settled it into the terminal state its own enum declares, the retried create
/// mints a resource of its own.
///
/// The rows differ along what the rule generalises over: the KIND of resource
/// (which is what carries its own terminal state and its own delete), and the
/// name the retry asks for — the deleted resource's own name is free again, so
/// the retry may reuse it.
#[tokio::test]
async fn a_token_stops_replaying_a_resource_whose_delete_has_settled() {
    // (case, the resource kind, the name the retry asks for, that kind's
    //  terminal state)
    let rows: [(&str, Tombstoned, &str, &str); 6] = [
        ("an SdiSource, retried under a new name", Tombstoned::SdiSource, "source-b", DELETED),
        ("an SdiSource, retried under the deleted name", Tombstoned::SdiSource, "source-a", DELETED),
        ("a multiplex, retried under a new name", Tombstoned::Multiplex, "multiplex-b", DELETED),
        (
            "a multiplex, retried under the deleted name",
            Tombstoned::Multiplex,
            "multiplex-a",
            DELETED,
        ),
        ("a node, retried under a new name", Tombstoned::Node, "node-b", DEREGISTERED),
        ("a node, retried under the deleted name", Tombstoned::Node, "node-a", DEREGISTERED),
    ];

    for (case, kind, retried_name, terminal) in rows {
        let round = a_round(kind, case, retried_name).await;
        assert_eq!(
            round.replayed_id, round.first_id,
            "case {case:?}: expected a live resource's token to replay {:?}, got {:?}",
            round.first_id, round.replayed_id
        );
        assert_eq!(
            round.terminal_state, terminal,
            "case {case:?}: expected the settled delete to report {terminal}, got {:?}",
            round.terminal_state
        );
        assert_ne!(
            round.retried_id, round.first_id,
            "case {case:?}: expected a freshly minted resource, got the deleted {:?}",
            round.first_id
        );
        assert_ne!(
            round.retried_state, terminal,
            "case {case:?}: expected the retry to answer with a live resource, got {:?}",
            round.retried_state
        );
    }
}

/// Rule: a multiplex program's token is resolved through the multiplex the
/// request named, so once that multiplex has reached its terminal `DELETED`
/// state the token replays nothing either — and the create it no longer
/// short-circuits is then refused on the multiplex's state, which is the answer
/// any create addressed to a deleted multiplex gets, with nothing stored.
///
/// The rows differ along what the rule generalises over: the name the retry asks
/// for (the program's own and a new one) and the settings it carries.
#[tokio::test]
async fn a_program_token_stops_replaying_once_its_multiplex_is_deleted() {
    // (case, the program name the retry asks for, the program number its
    //  settings carry)
    let rows: [(&str, &str, i64); 3] = [
        ("the retry asks for the program's own name", "programme", 1),
        ("the retry asks for another name", "another", 2),
        ("the retry carries other settings", "another", 65_535),
    ];

    for (case, retried_name, program_number) in rows {
        let router = medialive();
        let multiplex_id = multiplex(&router, case, "doomed").await;
        // A program is created in a multiplex that is still coming up, but the
        // delete needs the stopped state, so the create's transition comes due
        // first.
        wait_for_the_clock();
        let token = "token-a";
        let (status, program) = create_program(
            &router,
            &multiplex_id,
            "programme",
            token,
            program_settings(0),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateMultiplexProgram: expected 201, got {status}: {program}"
        );

        // While the multiplex is live the token IS a replay: the same create
        // answers with the program it minted, whatever name it now asks for.
        let (status, replayed) = create_program(
            &router,
            &multiplex_id,
            retried_name,
            token,
            program_settings(program_number),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            member(&replayed, "programName"),
            json!("programme"),
            "case {case:?}: expected the token to replay the program it minted, got {replayed}"
        );

        let (status, deleting) = delete_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
        );
        wait_for_the_clock();
        let (status, settled) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {settled}"
        );
        assert_eq!(
            member(&settled, "state"),
            json!(DELETED),
            "case {case:?}: expected the deleted multiplex to report {DELETED}, got {settled}"
        );

        // The window is over, so the create is judged on the multiplex's state
        // like any other: refused, naming the multiplex, with nothing stored.
        let (status, refusal) = create_program(
            &router,
            &multiplex_id,
            retried_name,
            token,
            program_settings(program_number),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the retry past the delete to be refused, got {status}: \
             {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refusal}"
        );
        let message = error_message(&refusal);
        assert!(
            message.contains(&multiplex_id),
            "case {case:?}: expected the refusal to name the multiplex it refused, got \
             {message:?}"
        );
        assert_eq!(
            member(&settled, "programCount"),
            json!(1),
            "case {case:?}: expected the multiplex to still hold its one program, got {settled}"
        );
        let (status, still_there) = describe_program(&router, &multiplex_id, "programme").await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplexProgram: expected 200, got {status}: {still_there}"
        );
    }
}
