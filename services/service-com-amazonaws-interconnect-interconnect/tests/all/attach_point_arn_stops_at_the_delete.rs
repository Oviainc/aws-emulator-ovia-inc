// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the STATE an `AttachPoint$arn` has to find behind it, driven
//! over the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: an attach point is a handle whose whole subject is the connectivity
//! a Connection is provisioned onto, so from the moment a `DeleteConnection` is made on
//! the connection behind such a handle the handle names nothing — the ARN is refused
//! with the operation's modelled 404, worded exactly as an ARN this scope never
//! allocated is, for the whole teardown as well as after it, while the record itself
//! stays readable by its own identifier so a caller polling the delete sees the
//! teardown. The readiness rule is the union's, not one member's: the gateway member is
//! refused once its gateway is no longer ready for use, and the ARN member is refused
//! once its connection's teardown has been called.
//!
//! The rows differ along what the rule generalises over: the operation that carries the
//! attach point (`smithy_query(query="operation",operation="CreateConnection,AcceptConnectionProposal")`
//! — the two inputs that carry an `AttachPoint`), WHEN in the teardown the handle is
//! offered (while the connection is being torn down, and once it has been), the Region
//! the request addresses and the environment and bandwidth it is placed on.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::attach_point_not_valid_message;

use crate::emulator::{code, message, placements_in, remote_account_for, text, Emulator};

/// The Regions the rows sign their requests for, so the rule is checked across the
/// Region the ARN carries as well as across the state of the connection it names.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// Long enough to cover a connection's tear-down span with margin, so a record whose
/// delete a row has called has reached its terminal state.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_millis(700);

/// When a row offers the ARN, relative to the `DeleteConnection` made on the connection
/// behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Offered {
    /// In the call right after the delete, while the teardown is still running.
    WhileTearingDown,
    /// Once the teardown has settled into the terminal state.
    OnceTornDown,
}

/// Every row's offering point, so each suite below spans the whole teardown.
const OFFERINGS: [Offered; 2] = [Offered::WhileTearingDown, Offered::OnceTornDown];

/// `CreateConnection` on `placement` with `attach_point`, signed for `region`, as a
/// `(status, body)` pair so a refusal is readable. The `remoteAccount` is taken from the
/// Environment's own record, so a row about the attach point is not also a row about the
/// identifying information.
async fn create_in(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
    attach_point: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(placement.0));
    request.insert("bandwidth".to_string(), json!(placement.1));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) =
        remote_account_for(&emulator.environment_in(region, &placement.0).await)
    {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A connection of `region`, created on one of the account's own Direct Connect
/// gateways — a live record whose ARN is an attach point until its delete is called.
async fn live_connection(emulator: &Emulator, region: &str, placement: &(String, String)) -> Value {
    let attach_point = json!({ "directConnectGateway": emulator.attach_point().await });
    let (status, body) = create_in(emulator, region, placement, &attach_point).await;
    assert_eq!(
        status, 200,
        "setup {region:?}: expected the connection the ARN will name to be created, got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// The `ConnectionId`s `region`'s endpoint holds, walked page by page so the answer is
/// the whole store however it is windowed. Ids rather than whole records, because the
/// clock moves a record's `state` on its own between two listings.
async fn stored_ids(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok_in(region, "ListConnections", Value::Object(request))
            .await;
        all.extend(
            listed["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|connection| text(connection, "id")),
        );
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// Call `DeleteConnection` on `identifier` and, per `offering`, let the teardown settle,
/// so the row is ready to offer that connection's ARN as an attach point.
async fn delete_and_reach(emulator: &Emulator, region: &str, identifier: &str, offering: Offered) {
    let deleted = emulator
        .ic_ok_in(
            region,
            "DeleteConnection",
            json!({ "identifier": identifier }),
        )
        .await;
    assert_eq!(
        text(&deleted["connection"], "state"),
        "deleting",
        "setup {identifier:?}: expected the delete to arm the teardown, got {:?}",
        deleted["connection"]["state"]
    );
    if offering == Offered::OnceTornDown {
        tokio::time::sleep(SETTLE_WAIT).await;
    }
}

/// The `ConnectionState` the record reports NOW, read through the implemented reader.
///
/// Read right AFTER the call a row is judging, so a `deleting` answer here says the row's
/// own call was made while the teardown was still running — the clock only moves a
/// connection's state forward, so a record still being torn down now was being torn down
/// then. The read also pins the other half of the rule: the record stays readable BY ITS
/// IDENTIFIER however the handle answers, which is how a caller polls the teardown and
/// then the terminal state.
async fn state_now(emulator: &Emulator, region: &str, identifier: &str) -> String {
    let (status, read) = emulator
        .ic_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    assert_eq!(
        status, 200,
        "case {identifier:?}: expected the deleted connection to stay readable by its identifier, got {status} with {read:?}"
    );
    text(&read["connection"], "state")
}

/// Assert `(status, body)` is the modelled 404 this service answers an attach point that
/// resolves to nothing with, worded for `arn` on `environment_id`.
fn assert_not_an_attach_point(
    case: &str,
    arn: &str,
    environment_id: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404 for an attach point whose delete has been called, got {status} with {body:?}"
    );
    assert!(
        code(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        code(body)
    );
    assert_eq!(
        message(body),
        attach_point_not_valid_message(arn, environment_id),
        "case {case:?}: expected this service's own wording for an attach point that resolves to nothing, got {:?}",
        message(body)
    );
}

// ---------------------------------------------------------------------------
// CreateConnection
// ---------------------------------------------------------------------------

/// RULE: a `CreateConnection` whose `attachPoint.arn` names a connection a
/// `DeleteConnection` has been made on is refused with the modelled 404 and provisions
/// nothing — in the call right after the delete as well as once the teardown has
/// settled — while the ARN of a connection with no delete against it is accepted.
#[tokio::test]
async fn a_create_refuses_an_attach_point_arn_whose_delete_was_called() {
    let emulator = Emulator::new();
    let mut torn_down_mid_flight = 0usize;

    for (seed, offering) in OFFERINGS.into_iter().enumerate() {
        for (offset, region) in REGIONS.iter().enumerate() {
            // The row's placement comes from the catalog of its OWN Region: an
            // Environment belongs to the Region whose endpoint offers it.
            let places = placements_in(&emulator, region).await;
            let placement = &places[(seed * REGIONS.len() + offset) % places.len()];
            let case = format!("{offering:?} / {region} / {} / {}", placement.0, placement.1);

            // Two connections of this Region on the same placement: one whose delete is
            // called, one left alone, so the only difference between the refusal and the
            // acceptance below is the state of the connection the ARN names.
            let doomed = live_connection(&emulator, region, placement).await;
            let untouched = live_connection(&emulator, region, placement).await;
            let doomed_arn = text(&doomed, "arn");
            let untouched_arn = text(&untouched, "arn");
            let doomed_id = text(&doomed, "id");

            // Taken BEFORE the delete, which leaves the record in the store (that is
            // what a caller polling the teardown reads), so this is the id set the
            // refused create below must not change.
            let before = stored_ids(&emulator, region).await;
            delete_and_reach(&emulator, region, &doomed_id, offering).await;

            let (status, body) =
                create_in(&emulator, region, placement, &json!({ "arn": doomed_arn })).await;
            assert_not_an_attach_point(&case, &doomed_arn, &placement.0, status, &body);

            // Read right after the refusal, so a `deleting` answer says THAT create was
            // refused inside the teardown window rather than after it.
            let state = state_now(&emulator, region, &doomed_id).await;
            assert!(
                matches!(state.as_str(), "deleting" | "deleted"),
                "case {case:?}: expected the connection behind the ARN to be torn down or gone, got {state:?}"
            );
            if state == "deleting" {
                torn_down_mid_flight += 1;
            }

            let after = stored_ids(&emulator, region).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused create to have provisioned nothing, got {after:?}"
            );

            // The ARN of a connection with no delete against it is still an attach
            // point, so what refused the request above is the STATE behind the handle.
            let (status, attached) =
                create_in(&emulator, region, placement, &json!({ "arn": untouched_arn })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the ARN of a live connection to be accepted, got {status} with {attached:?}"
            );
            let fetched = emulator
                .ic_ok_in(
                    region,
                    "GetConnection",
                    json!({ "identifier": text(&attached["connection"], "id") }),
                )
                .await;
            assert_eq!(
                fetched["connection"]["attachPoint"],
                json!({ "arn": untouched_arn }),
                "case {case:?}: expected GetConnection to report the ARN attach point, got {:?}",
                fetched["connection"]["attachPoint"]
            );
        }
    }

    assert!(
        torn_down_mid_flight > 0,
        "case \"teardown window\": expected at least one row to be refused while the connection was still being torn down, got {torn_down_mid_flight}"
    );
}

/// RULE: while the connection behind an attach-point ARN is being torn down its record
/// is still readable by that same ARN — the handle stops resolving, the resource does
/// not — so the refusal above is about what the handle names and not about an ARN the
/// store cannot resolve at all.
#[tokio::test]
async fn a_connection_being_torn_down_is_still_readable_by_its_arn() {
    let emulator = Emulator::new();
    let mut rows_inside_the_window = 0usize;

    for (seed, region) in REGIONS.iter().enumerate() {
        let places = placements_in(&emulator, region).await;
        let placement = &places[seed % places.len()];
        let case = format!("{region} / {} / {}", placement.0, placement.1);
        let doomed = live_connection(&emulator, region, placement).await;
        let arn = text(&doomed, "arn");
        let id = text(&doomed, "id");

        emulator
            .ic_ok_in(region, "DeleteConnection", json!({ "identifier": id }))
            .await;

        // Offered as an attach point: refused, because the connectivity it is the handle
        // for is going. True for the whole teardown and after it, so this row asserts it
        // whatever the clock has done by now.
        let (status, body) = create_in(&emulator, region, placement, &json!({ "arn": arn })).await;
        assert_not_an_attach_point(&case, &arn, &placement.0, status, &body);

        // Used as the resource's own identity, while the teardown is still running: the
        // ARN still resolves, because the record is there to be read. Judged against the
        // state the record reports after the read, so a row the clock has already settled
        // is not asked a question about the window it has left.
        let (status, tags) = emulator
            .ic_in(region, "ListTagsForResource", json!({ "arn": arn }))
            .await;
        if state_now(&emulator, region, &id).await == "deleting" {
            rows_inside_the_window += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the record to stay readable by its ARN while it is torn down, got {status} with {tags:?}"
            );
        }
    }

    assert!(
        rows_inside_the_window > 0,
        "case \"teardown window\": expected at least one row to read the record while it was still being torn down, got {rows_inside_the_window}"
    );
}

// ---------------------------------------------------------------------------
// AcceptConnectionProposal
// ---------------------------------------------------------------------------

/// RULE: the same rule decides the attach point an ACCEPTANCE selects — an ARN naming a
/// connection whose delete has been called resolves to nothing, leaving the proposal
/// exactly as it was, while an ARN with no delete against it is accepted.
#[tokio::test]
async fn an_acceptance_refuses_an_attach_point_arn_whose_delete_was_called() {
    let emulator = Emulator::new();

    for (seed, offering) in OFFERINGS.into_iter().enumerate() {
        for (offset, region) in REGIONS.iter().enumerate() {
            let places = placements_in(&emulator, region).await;
            let placement = &places[(seed * REGIONS.len() + offset + 1) % places.len()];
            let case = format!("{offering:?} / {region} / {} / {}", placement.0, placement.1);

            let doomed = live_connection(&emulator, region, placement).await;
            let untouched = live_connection(&emulator, region, placement).await;
            let doomed_arn = text(&doomed, "arn");
            let untouched_arn = text(&untouched, "arn");
            delete_and_reach(&emulator, region, &text(&doomed, "id"), offering).await;

            // One proposal per acceptance, each accepted in the call right after the
            // create: a proposal is only awaiting acceptance until the clock hands it on.
            let proposal = live_connection(&emulator, region, placement).await;
            let (status, body) = emulator
                .ic_in(
                    region,
                    "AcceptConnectionProposal",
                    json!({
                        "activationKey": text(&proposal, "activationKey"),
                        "attachPoint": { "arn": doomed_arn },
                    }),
                )
                .await;
            assert_not_an_attach_point(&case, &doomed_arn, &placement.0, status, &body);

            // The refusal associated nothing: the proposal still carries the attach
            // point it was created on.
            let unchanged = emulator
                .ic_ok_in(
                    region,
                    "GetConnection",
                    json!({ "identifier": text(&proposal, "id") }),
                )
                .await;
            assert_eq!(
                unchanged["connection"]["attachPoint"], proposal["attachPoint"],
                "case {case:?}: expected the refused acceptance to leave the proposal's attach point alone, got {:?}",
                unchanged["connection"]["attachPoint"]
            );

            // An ARN with no delete against it is accepted, so what refused the
            // acceptance above is the STATE behind the handle.
            let fresh = live_connection(&emulator, region, placement).await;
            let (status, accepted) = emulator
                .ic_in(
                    region,
                    "AcceptConnectionProposal",
                    json!({
                        "activationKey": text(&fresh, "activationKey"),
                        "attachPoint": { "arn": untouched_arn },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the ARN of a live connection to be accepted, got {status} with {accepted:?}"
            );
            assert_eq!(
                accepted["connection"]["attachPoint"],
                json!({ "arn": untouched_arn }),
                "case {case:?}: expected the accepted connection to carry the ARN attach point, got {:?}",
                accepted["connection"]["attachPoint"]
            );
        }
    }
}
