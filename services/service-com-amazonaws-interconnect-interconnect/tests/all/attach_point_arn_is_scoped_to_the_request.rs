// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the SCOPE an attach-point ARN is resolved in, driven over the
//! wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: an `AttachPoint$arn` is resolved against the connections the Region
//! the REQUEST addressed holds — a connection belongs to the Region it was created in —
//! so the ARN of a live connection is an attach point only through that connection's own
//! regional endpoint, and through every other Region's endpoint it resolves to nothing
//! and is refused with the operation's modelled 404, worded exactly as an ARN this scope
//! never allocated is, having provisioned and associated nothing.
//!
//! The rows differ in the Region the connection the ARN names lives in, the Region the
//! request is signed for (including a Region holding nothing at all), the environment and
//! bandwidth the request is placed on, and the operation that carries the attach point
//! (`smithy_query(query="operation",operation="CreateConnection,AcceptConnectionProposal")`
//! — the two inputs that carry an `AttachPoint`).

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::attach_point_not_valid_message;

use crate::emulator::{
    code, message, placements, remote_account_for, text, Emulator, DEFAULT_SIGNING_REGION,
    SIGNING_NAME,
};

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The partition every ARN this emulator mints carries.
const PARTITION: &str = "aws";

/// The Regions a row's connection lives in: the Region the rest of this crate's suites
/// sign for, plus three more on three other continents, so a row's Region is never the
/// harness default by accident.
const LIVE_REGIONS: [&str; 4] = [
    DEFAULT_SIGNING_REGION,
    "eu-west-1",
    "ap-southeast-2",
    "sa-east-1",
];

/// A Region no row creates anything in, so every ARN is foreign to its endpoint.
const EMPTY_REGION: &str = "eu-central-1";

/// A connection ARN with the fields a row wants, built the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(region: &str, id: &str) -> String {
    format!("arn:{PARTITION}:{SIGNING_NAME}:{region}:{ACCOUNT_ID}:connection/{id}")
}

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
    if let Some(remote_account) = remote_account_for(&emulator.environment(&placement.0).await) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A connection of `region`, created on one of the account's own Direct Connect
/// gateways — the live record whose ARN the rows below offer as an attach point.
async fn live_connection(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
) -> Value {
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
        "case {case:?}: expected the modelled 404 for an attach point that resolves to nothing, got {status} with {body:?}"
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

/// RULE: the ARN of a LIVE connection is an attach point only through the endpoint of the
/// Region that connection lives in — every other Region's endpoint resolves it to
/// nothing, exactly as it resolves an ARN of its own Region that was never allocated, and
/// provisions nothing for it.
#[tokio::test]
async fn an_attach_point_arn_resolves_only_through_the_region_its_connection_lives_in() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;

    for (seed, region) in LIVE_REGIONS.iter().enumerate() {
        let placement = &places[seed % places.len()];
        let live = live_connection(&emulator, region, placement).await;
        let arn = text(&live, "arn");
        let id = text(&live, "id");
        assert_eq!(
            arn,
            connection_arn(region, &id),
            "case {region:?}: expected the ARN to carry the Region the create was signed for, got {arn:?}"
        );

        let elsewhere: Vec<&str> = LIVE_REGIONS
            .iter()
            .copied()
            .chain(std::iter::once(EMPTY_REGION))
            .filter(|caller| caller != region)
            .collect();
        for (offset, caller) in elsewhere.iter().enumerate() {
            // The row's placement varies with the caller too, so the rule is not
            // checked on one environment and bandwidth only.
            let caller_placement = &places[(seed + offset + 1) % places.len()];
            let before = stored_ids(&emulator, caller).await;

            // The live connection's own ARN, offered as an attach point through
            // another Region's endpoint: that endpoint holds no such connection.
            let case = format!("{arn} offered in {caller} on CreateConnection");
            let (status, body) =
                create_in(&emulator, caller, caller_placement, &json!({ "arn": arn })).await;
            assert_not_an_attach_point(&case, &arn, &caller_placement.0, status, &body);

            // The SAME id, carried by an ARN of the Region the request addresses: an
            // ARN this scope never allocated, which is refused with the same status,
            // the same code and the same wording — so what refuses the request never
            // reports whether another scope holds the resource.
            let never_allocated = connection_arn(caller, &id);
            let (other_status, other_body) = create_in(
                &emulator,
                caller,
                caller_placement,
                &json!({ "arn": never_allocated }),
            )
            .await;
            assert_not_an_attach_point(
                &format!("{never_allocated} offered in {caller} on CreateConnection"),
                &never_allocated,
                &caller_placement.0,
                other_status,
                &other_body,
            );
            assert_eq!(
                (status, code(&body)),
                (other_status, code(&other_body)),
                "case {case:?}: expected an out-of-Region ARN to be indistinguishable from one this scope never allocated, got {:?} against {:?}",
                (status, code(&body)),
                (other_status, code(&other_body))
            );

            let after = stored_ids(&emulator, caller).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused creates to have provisioned nothing, got {after:?}"
            );
        }

        // And through the connection's OWN endpoint the very same ARN is an attach
        // point, read back through the implemented reader.
        let (status, attached) =
            create_in(&emulator, region, placement, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {arn:?} in {region}: expected the ARN of a connection of this Region to be accepted, got {status} with {attached:?}"
        );
        let attached_id = text(&attached["connection"], "id");
        let fetched = emulator
            .ic_ok_in(region, "GetConnection", json!({ "identifier": attached_id }))
            .await;
        assert_eq!(
            fetched["connection"]["attachPoint"],
            json!({ "arn": arn }),
            "case {arn:?} in {region}: expected GetConnection to report the ARN attach point, got {:?}",
            fetched["connection"]["attachPoint"]
        );
    }
}

// ---------------------------------------------------------------------------
// AcceptConnectionProposal
// ---------------------------------------------------------------------------

/// RULE: the same scope decides the attach point an ACCEPTANCE selects — an ARN naming a
/// live connection of another Region resolves to nothing there, leaving the proposal
/// exactly as it was, while an ARN of the Region the request addresses is accepted.
#[tokio::test]
async fn an_acceptance_resolves_an_attach_point_arn_in_the_region_it_addresses() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;

    // Rows differ in the Region the ARN's connection lives in, the Region the
    // acceptance is made in, and the placement each one sits on.
    let rows: Vec<(&str, &str)> = vec![
        (LIVE_REGIONS[0], LIVE_REGIONS[1]),
        (LIVE_REGIONS[2], EMPTY_REGION),
    ];

    for (seed, (home, caller)) in rows.into_iter().enumerate() {
        let home_placement = &places[seed % places.len()];
        let caller_placement = &places[(seed + 1) % places.len()];
        let foreign_arn = text(&live_connection(&emulator, home, home_placement).await, "arn");
        let local_arn = text(
            &live_connection(&emulator, caller, caller_placement).await,
            "arn",
        );

        // One proposal per acceptance, each accepted in the call right after the
        // create: a proposal is only awaiting acceptance until the clock hands it on.
        let proposal = live_connection(&emulator, caller, caller_placement).await;
        let case = format!("{foreign_arn} offered in {caller} on AcceptConnectionProposal");
        let (status, body) = emulator
            .ic_in(
                caller,
                "AcceptConnectionProposal",
                json!({
                    "activationKey": text(&proposal, "activationKey"),
                    "attachPoint": { "arn": foreign_arn },
                }),
            )
            .await;
        assert_not_an_attach_point(&case, &foreign_arn, &caller_placement.0, status, &body);

        // The refusal associated nothing: the proposal still carries the attach point
        // it was created on.
        let unchanged = emulator
            .ic_ok_in(
                caller,
                "GetConnection",
                json!({ "identifier": text(&proposal, "id") }),
            )
            .await;
        assert_eq!(
            unchanged["connection"]["attachPoint"], proposal["attachPoint"],
            "case {case:?}: expected the refused acceptance to leave the proposal's attach point alone, got {:?}",
            unchanged["connection"]["attachPoint"]
        );

        // An ARN of the Region the acceptance addresses does resolve, and the accepted
        // connection reports it.
        let accepted_proposal = live_connection(&emulator, caller, caller_placement).await;
        let (status, accepted) = emulator
            .ic_in(
                caller,
                "AcceptConnectionProposal",
                json!({
                    "activationKey": text(&accepted_proposal, "activationKey"),
                    "attachPoint": { "arn": local_arn },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {local_arn:?} in {caller}: expected an ARN of this Region to be accepted, got {status} with {accepted:?}"
        );
        assert_eq!(
            accepted["connection"]["attachPoint"],
            json!({ "arn": local_arn }),
            "case {local_arn:?} in {caller}: expected the accepted connection to carry the ARN attach point, got {:?}",
            accepted["connection"]["attachPoint"]
        );
    }
}
