// SPDX-License-Identifier: CC0-1.0

//! What an `AttachPoint` member may CARRY, driven over the wire through the shared
//! `crate::emulator` harness.
//!
//! RULE: the value an `AttachPoint` carries is held to its OWN member's modelled
//! `@length` and `@pattern` by every operation the union reaches, and a value outside
//! them is a fault in the REQUEST — the modelled `InterconnectValidationException`
//! (400), worded the way this service words every value that is not an identifier of its
//! kind — rather than an identifier resolved against state and then answered as a
//! resource the account does not hold (404) or as an empty page. A value INSIDE those
//! constraints is resolved as before: one that names nothing is still the modelled 404.
//!
//! The constraints are the model's, read from
//! `smithy_query(query="shape",shape="AttachPoint")`: `arn` carries `@length(59, 150)`
//! with `@pattern ^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`,
//! and `directConnectGateway` carries `@length(36, 36)` with
//! `@pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
//!
//! The rows differ along what the rule generalises over: the member the value arrived
//! in, the WAY the value leaves that member's constraints (its length at and past each
//! published bound, its field structure, the alphabet of a field, the resource id it ends
//! in), the operation the union arrived on, the environment, the bandwidth and the
//! Region the request is signed for.

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    attach_point_not_valid_message, not_a_connection_arn_message,
    not_a_gateway_identifier_message,
};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text,
    Emulator, SIGNING_NAME,
};

/// The two Regions the rows sign their requests for, so the rule is checked across the
/// Region the request addresses as well as across the value.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The account every ARN this emulator mints carries, and so the account field a
/// well-formed row carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `@length` the model gives `AttachPoint$arn` (`smithy_query(query="shape",shape="AttachPoint")`).
const ARN_LENGTH: (usize, usize) = (59, 150);

/// The `@length` the model gives `AttachPoint$directConnectGateway` — one value, because
/// the member is modelled `@length(36, 36)`.
const GATEWAY_LENGTH: usize = 36;

/// A gateway id of the modelled length, spelled the way the member's pattern spells one.
/// The account does not hold it, so it is well-formed and names nothing.
const WELL_FORMED_GATEWAY: &str = "00000000-0000-4000-8000-000000000000";

/// A second well-formed gateway id, spanning the hex LETTERS the member's pattern
/// allows as well as its digits — so a row that varies the case of a value varies
/// something.
const WELL_FORMED_HEX_GATEWAY: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// The error the service answers a request whose input it cannot accept with.
const VALIDATION_CODE: &str = "InterconnectValidationException";

/// The error the service answers an identifier that names no attach point with.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

// ---------------------------------------------------------------------------
// The values the rows send
// ---------------------------------------------------------------------------

/// A connection ARN carrying the fields a row wants, spelled the way
/// `AttachPoint$arn`'s pattern spells one:
/// `arn:aws:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(region: &str, account: &str, id: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// An otherwise well-formed connection ARN of exactly `length` characters.
///
/// Every field but the Region is fixed, and the pattern lets the Region field be any
/// run of non-colon characters, so the length a row wants is reached by padding that
/// field alone.
fn arn_of_length(length: usize, id: &str) -> String {
    let fixed = connection_arn("", ACCOUNT_ID, id).chars().count();
    connection_arn(
        &"r".repeat(length.saturating_sub(fixed)),
        ACCOUNT_ID,
        id,
    )
}

/// A hyphenated gateway id of `length` characters: the modelled spelling, with its last
/// group shortened or lengthened so only the LENGTH differs from a well-formed one.
fn gateway_of_length(length: usize) -> String {
    let head = "00000000-0000-4000-8000-";
    let tail = length.saturating_sub(head.chars().count());
    format!("{head}{}", "0".repeat(tail))
}

/// One row: what makes the value impossible for its member, and the `attachPoint` to
/// send.
struct Row {
    name: String,
    attach_point: Value,
}

fn row(name: &str, attach_point: Value) -> Row {
    Row {
        name: name.to_string(),
        attach_point,
    }
}

/// The identifier an `attachPoint` carries, whichever member carries it.
fn identifier_of(attach_point: &Value) -> String {
    attach_point
        .as_object()
        .and_then(|members| members.values().next())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// How the service words the refusal of this row, which is the sentence the crate owns
/// for a value that is not an identifier of that member's kind at all.
fn expected_message(attach_point: &Value) -> String {
    let identifier = identifier_of(attach_point);
    if attach_point.get("arn").is_some() {
        not_a_connection_arn_message(&identifier)
    } else {
        not_a_gateway_identifier_message(&identifier)
    }
}

/// Values no `AttachPoint` member may carry: each one leaves its member's own `@length`
/// or `@pattern` in a different way.
fn impossible_values() -> Vec<Row> {
    let well_formed_id = "mcc-00000000";
    let well_formed = connection_arn(REGIONS[0], ACCOUNT_ID, well_formed_id);
    vec![
        // `arn`: the pattern's fields.
        row("an arn that is not an ARN at all", json!({ "arn": "not-an-arn" })),
        row("an empty arn", json!({ "arn": "" })),
        row(
            "an arn of another service's resource",
            json!({ "arn": well_formed.replace(SIGNING_NAME, "directconnect") }),
        ),
        row(
            "an arn of another resource type",
            json!({ "arn": well_formed.replace("connection/", "environment/") }),
        ),
        row(
            "an arn whose account field is not twelve digits",
            json!({ "arn": connection_arn(REGIONS[0], "12345", well_formed_id) }),
        ),
        row(
            "an arn carrying a field past the resource one",
            json!({ "arn": format!("{well_formed}:extra") }),
        ),
        // `arn`: the resource id the pattern ends in.
        row(
            "an arn whose id prefix is neither of the two the pattern allows",
            json!({ "arn": connection_arn(REGIONS[0], ACCOUNT_ID, "xcc-00000000") }),
        ),
        row(
            "an arn whose id is upper-cased",
            json!({ "arn": connection_arn(REGIONS[0], ACCOUNT_ID, "MCC-0A1B2C3D") }),
        ),
        row(
            "an arn whose id suffix is one character short",
            json!({ "arn": connection_arn(REGIONS[0], ACCOUNT_ID, "mcc-0000000") }),
        ),
        row(
            "an arn whose id suffix is one character long",
            json!({ "arn": connection_arn(REGIONS[0], ACCOUNT_ID, "lmcc-000000000") }),
        ),
        // `arn`: one character past each published length bound. No ARN the pattern
        // admits is shorter than the floor — the shortest one has a single-character
        // Region field — so the row below the floor is that ARN with the field gone.
        row(
            "an arn one character below the modelled length floor",
            json!({ "arn": arn_of_length(ARN_LENGTH.0 - 1, well_formed_id) }),
        ),
        row(
            "an arn one character past the modelled length ceiling",
            json!({ "arn": arn_of_length(ARN_LENGTH.1 + 1, well_formed_id) }),
        ),
        // `directConnectGateway`: the pattern's groups and alphabet.
        row(
            "a gateway id that is not a UUID at all",
            json!({ "directConnectGateway": "abc" }),
        ),
        row("an empty gateway id", json!({ "directConnectGateway": "" })),
        row(
            "a gateway id with a character outside the hex alphabet",
            json!({ "directConnectGateway": "0000000g-0000-4000-8000-000000000000" }),
        ),
        row(
            "a gateway id of the modelled length with its hyphens in the wrong places",
            json!({ "directConnectGateway": "000000000-000-4000-8000-000000000000" }),
        ),
        row(
            "a gateway id with its hyphens stripped",
            json!({ "directConnectGateway": WELL_FORMED_GATEWAY.replace('-', "") }),
        ),
        row(
            "a gateway id wrapped in braces",
            json!({ "directConnectGateway": format!("{{{WELL_FORMED_GATEWAY}}}") }),
        ),
        // `directConnectGateway`: one character past each side of its single length.
        row(
            "a gateway id one character short of the modelled length",
            json!({ "directConnectGateway": gateway_of_length(GATEWAY_LENGTH - 1) }),
        ),
        row(
            "a gateway id one character past the modelled length",
            json!({ "directConnectGateway": gateway_of_length(GATEWAY_LENGTH + 1) }),
        ),
    ]
}

/// Values every member constraint admits that still name no attach point of the
/// account: at each published length bound, in each id prefix, and in each member.
fn well_formed_values_naming_nothing() -> Vec<Row> {
    vec![
        row(
            "an arn at the modelled length floor",
            json!({ "arn": arn_of_length(ARN_LENGTH.0, "mcc-00000000") }),
        ),
        row(
            "an arn at the modelled length ceiling",
            json!({ "arn": arn_of_length(ARN_LENGTH.1, "lmcc-zzzzzzzz") }),
        ),
        row(
            "an arn of the other id prefix, naming nothing",
            json!({ "arn": connection_arn(REGIONS[0], ACCOUNT_ID, "lmcc-a1b2c3d4") }),
        ),
        row(
            "a gateway id at the modelled length the account does not hold",
            json!({ "directConnectGateway": WELL_FORMED_GATEWAY }),
        ),
        row(
            "an upper-cased gateway id the account does not hold",
            json!({ "directConnectGateway": WELL_FORMED_HEX_GATEWAY.to_ascii_uppercase() }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The operations the union reaches
// ---------------------------------------------------------------------------

/// `CreateConnection`, as a `(status, body)` pair so a refusal is readable.
async fn create(
    emulator: &Emulator,
    region: &str,
    environment: &Value,
    bandwidth: &str,
    attach_point: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert(
        "environmentId".to_string(),
        json!(text(environment, "environmentId")),
    );
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) = remote_account_for(environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A proposal awaiting acceptance, created on a gateway the account holds — the record
/// whose `activationKey` the acceptances below are made with.
async fn proposal(
    emulator: &Emulator,
    region: &str,
    environment: &Value,
    bandwidth: &str,
    gateway: &str,
) -> Value {
    let (status, body) = create(
        emulator,
        region,
        environment,
        bandwidth,
        &json!({ "directConnectGateway": gateway }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup {region:?}: expected the proposal to be created on {gateway:?}, got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// `AcceptConnectionProposal`, as a `(status, body)` pair.
async fn accept(
    emulator: &Emulator,
    region: &str,
    activation_key: &str,
    attach_point: &Value,
) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "AcceptConnectionProposal",
            json!({ "activationKey": activation_key, "attachPoint": attach_point }),
        )
        .await
}

/// `ListConnections` filtered by `attachPoint`, as a `(status, body)` pair.
async fn list_filtered(emulator: &Emulator, region: &str, attach_point: &Value) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "ListConnections",
            json!({ "maxResults": 100, "attachPoint": attach_point }),
        )
        .await
}

/// The ids of every connection this Region's endpoint holds.
async fn stored_ids(emulator: &Emulator, region: &str) -> Vec<String> {
    let listed = emulator
        .ic_ok_in(region, "ListConnections", json!({ "maxResults": 100 }))
        .await;
    listed["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| text(connection, "id"))
        .collect()
}

/// The `(environment, bandwidth, region)` rows the tables are driven over: one row per
/// environment that takes a connection, with the bandwidth and the Region varying with
/// it so the rule is never checked on one placement only.
///
/// Each row carries the Environment ITS OWN Region's endpoint offers for that partner
/// offering, because an Environment belongs to the Region whose connections it
/// produces.
async fn placements(emulator: &Emulator) -> Vec<(Value, String, &'static str)> {
    let catalog = emulator.environments().await;
    let mut out = Vec::new();
    for environment in creatable_environments(&catalog) {
        let bandwidths = available_bandwidths(&environment);
        let bandwidth = bandwidths[out.len() % bandwidths.len()].clone();
        let region = REGIONS[out.len() % REGIONS.len()];
        out.push((
            emulator.counterpart_in(region, &environment).await,
            bandwidth,
            region,
        ));
    }
    assert!(
        !out.is_empty(),
        "case \"catalog\": expected at least one environment that takes connections"
    );
    out
}

// ---------------------------------------------------------------------------
// A value outside the member's own constraints
// ---------------------------------------------------------------------------

/// RULE: a value an `AttachPoint` member cannot hold is refused by BOTH writers of a
/// connection's attach point as invalid input — HTTP 400 with the modelled
/// `InterconnectValidationException`, worded as the value not being an identifier of
/// that member's kind — and neither writer resolves it, provisions anything or touches
/// the proposal it was offered to.
#[tokio::test]
async fn a_value_outside_an_attach_point_members_constraints_is_invalid_input() {
    let emulator = Emulator::new();
    // A gateway the account holds, so what the rows below refuse is the value and not an
    // account that holds no attach point at all.
    let held = emulator.attach_point().await;

    let rows = impossible_values();
    assert!(
        rows.iter()
            .any(|row| row.attach_point.get("arn").is_some())
            && rows
                .iter()
                .any(|row| row.attach_point.get("directConnectGateway").is_some()),
        "case \"table\": expected rows for both members, got {} rows",
        rows.len()
    );

    for (environment, bandwidth, region) in placements(&emulator).await {
        let environment_id = text(&environment, "environmentId");
        let before = stored_ids(&emulator, region).await;
        // ONE proposal per placement: every acceptance below is refused, so none of them
        // consumes it.
        let awaiting = proposal(&emulator, region, &environment, &bandwidth, &held).await;
        let activation_key = text(&awaiting, "activationKey");

        for Row { name, attach_point } in &rows {
            let case = format!("{environment_id} / {bandwidth} / {region} / {name}");
            let (created_status, created_body) =
                create(&emulator, region, &environment, &bandwidth, attach_point).await;
            let (accepted_status, accepted_body) =
                accept(&emulator, region, &activation_key, attach_point).await;

            for (operation, status, body) in [
                ("CreateConnection", created_status, &created_body),
                ("AcceptConnectionProposal", accepted_status, &accepted_body),
            ] {
                assert_eq!(
                    status, 400,
                    "case {case:?} / {operation}: expected HTTP 400 for a value the member cannot hold, got {status} with {body:?}"
                );
                assert!(
                    code(body).contains(VALIDATION_CODE),
                    "case {case:?} / {operation}: expected {VALIDATION_CODE}, got {:?} with {body:?}",
                    code(body)
                );
                assert_eq!(
                    message(body),
                    expected_message(attach_point),
                    "case {case:?} / {operation}: expected the invalid-value wording, got {:?}",
                    message(body)
                );
                assert!(
                    !message(body).contains(&environment_id),
                    "case {case:?} / {operation}: expected the refusal to be about the value and not about the environment it was offered for, got {:?}",
                    message(body)
                );
            }
        }

        // Not one of those refusals provisioned anything, and the proposal still carries
        // the attach point it was created on.
        let mut after = stored_ids(&emulator, region).await;
        after.retain(|id| id != &text(&awaiting, "id"));
        assert_eq!(
            after, before,
            "case {environment_id:?}: expected the refused requests to have provisioned nothing, got {after:?}"
        );
        let stored = emulator
            .ic_ok_in(
                region,
                "GetConnection",
                json!({ "identifier": text(&awaiting, "id") }),
            )
            .await;
        assert_eq!(
            stored["connection"]["attachPoint"], awaiting["attachPoint"],
            "case {environment_id:?}: expected the refused acceptances to leave the proposal's attach point alone, got {:?}",
            stored["connection"]["attachPoint"]
        );
    }
}

// ---------------------------------------------------------------------------
// A value inside the member's constraints
// ---------------------------------------------------------------------------

/// RULE: a value every one of its member's constraints admits is still RESOLVED — one
/// that names no attach point valid for the Environment is the modelled 404, naming the
/// value and the Environment — so the 400s above are about the value's own spelling and
/// not a blanket refusal, and a gateway the account holds is accepted by both writers.
#[tokio::test]
async fn a_well_formed_value_that_names_nothing_is_still_the_not_found() {
    let emulator = Emulator::new();
    let held = emulator.attach_point().await;

    for (environment, bandwidth, region) in placements(&emulator).await {
        let environment_id = text(&environment, "environmentId");
        let awaiting = proposal(&emulator, region, &environment, &bandwidth, &held).await;
        let activation_key = text(&awaiting, "activationKey");

        for Row { name, attach_point } in &well_formed_values_naming_nothing() {
            let case = format!("{environment_id} / {bandwidth} / {region} / {name}");
            let (created_status, created_body) =
                create(&emulator, region, &environment, &bandwidth, attach_point).await;
            let (accepted_status, accepted_body) =
                accept(&emulator, region, &activation_key, attach_point).await;

            for (operation, status, body) in [
                ("CreateConnection", created_status, &created_body),
                ("AcceptConnectionProposal", accepted_status, &accepted_body),
            ] {
                assert_eq!(
                    status, 404,
                    "case {case:?} / {operation}: expected the modelled 404 for a well-formed value naming nothing, got {status} with {body:?}"
                );
                assert!(
                    code(body).contains(NOT_FOUND_CODE),
                    "case {case:?} / {operation}: expected {NOT_FOUND_CODE}, got {:?} with {body:?}",
                    code(body)
                );
                assert_eq!(
                    message(body),
                    attach_point_not_valid_message(&identifier_of(attach_point), &environment_id),
                    "case {case:?} / {operation}: expected the not-an-attach-point wording, got {:?}",
                    message(body)
                );
            }
        }

        // Positive control: the gateway the account HOLDS is accepted, so the rows above
        // are refused for what they carry.
        let (status, body) = accept(
            &emulator,
            region,
            &activation_key,
            &json!({ "directConnectGateway": held }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {environment_id:?}: expected the held gateway to be accepted, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The same constraints on the list filter
// ---------------------------------------------------------------------------

/// RULE: the `ListConnections` `attachPoint` filter carries the same union, so it holds
/// the same member constraints — a value the member cannot hold is the same 400, worded
/// the same way, rather than a page the caller cannot tell from "nothing matched" —
/// while a well-formed value is filtered on: one that matches nothing answers an empty
/// page, and a stored connection's own attach point answers that connection.
#[tokio::test]
async fn the_list_filter_holds_the_same_member_constraints() {
    let emulator = Emulator::new();
    let held = emulator.attach_point().await;
    let placements = placements(&emulator).await;
    let (environment, bandwidth, region) = placements[0].clone();
    let live = proposal(&emulator, region, &environment, &bandwidth, &held).await;

    for Row { name, attach_point } in &impossible_values() {
        let case = format!("{region} / {name}");
        let (status, body) = list_filtered(&emulator, region, attach_point).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a filter value the member cannot hold, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(VALIDATION_CODE),
            "case {case:?}: expected {VALIDATION_CODE}, got {:?} with {body:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            expected_message(attach_point),
            "case {case:?}: expected the invalid-value wording, got {:?}",
            message(&body)
        );
    }

    for Row { name, attach_point } in &well_formed_values_naming_nothing() {
        let case = format!("{region} / {name}");
        let (status, body) = list_filtered(&emulator, region, attach_point).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a well-formed filter to be served, got {status} with {body:?}"
        );
        assert_eq!(
            body["connections"].as_array().map(Vec::len).unwrap_or(0),
            0,
            "case {case:?}: expected no connection to match a value nothing holds, got {:?}",
            body["connections"]
        );
    }

    // The filter does select on a value a stored connection holds, so the empty pages
    // above are about the value and not about the filter being ignored.
    let (status, body) = list_filtered(
        &emulator,
        region,
        &json!({ "directConnectGateway": held }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the live connection's own attach point\": expected the filter to be served, got {status} with {body:?}"
    );
    let matched: Vec<String> = body["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| text(connection, "id"))
        .collect();
    assert!(
        matched.contains(&text(&live, "id")),
        "case \"the live connection's own attach point\": expected the stored connection to match, got {matched:?}"
    );
}
