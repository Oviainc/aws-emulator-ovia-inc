// SPDX-License-Identifier: CC0-1.0

//! What the slice answers when a request names a resource that is not stored, over the
//! wire. Two rules, each asserted over a table that spans the class rather than the one
//! identifier a recording happened to carry:
//!
//! * A REPORTING operation whose output is a paginated collection answers an identifier
//!   that selects members of that collection but matches nothing stored with HTTP 200
//!   and that collection EMPTY — `DescribeFleetAttributes`' own doc: "a FleetAttributes
//!   object is returned for each fleet requested, unless the fleet identifier is not
//!   found", and "Attribute objects are returned only for fleets that currently exist".
//!   So an identifier that is not found is simply absent from the page.
//! * An operation that must ACT ON a named resource answers the modelled
//!   `NotFoundException` (HTTP 400) whose `Message` is BUILT FROM THE REQUEST: the
//!   identifier the caller supplied, verbatim, in that operation's own template.
//!
//! The harness (`GameLift`, `create_fleet`, …) is the one `hosting_loop.rs` defines.

use serde_json::{json, Value};

use crate::hosting_loop::{
    create_fleet, ec2_fleet, gamelift_arn, wait_for_due_transitions, GameLift, REGION,
};

/// The `Message` member of a modelled error body. awsJson renders the member under its
/// modelled name (`NotFoundException$Message`); the lower-case spelling is accepted too
/// so this helper reads the body whichever casing the protocol layer emits.
fn error_message(body: &Value) -> String {
    for key in ["Message", "message"] {
        if let Some(message) = body.get(key).and_then(Value::as_str) {
            return message.to_string();
        }
    }
    format!("<no Message in {body}>")
}

/// The `FleetId`s a `DescribeFleetAttributes` response reports, in order.
fn reported_fleet_ids(body: &Value) -> Vec<String> {
    body["FleetAttributes"]
        .as_array()
        .map(|attributes| {
            attributes
                .iter()
                .map(|attribute| attribute["FleetId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `DescribeFleetAttributes` reports the named fleets THAT EXIST: an identifier matching
/// no stored fleet is absent from the page, and the request still succeeds with 200 —
/// whether every identifier is unknown or only some of them are.
#[tokio::test]
async fn describing_unknown_fleets_reports_an_empty_page() {
    let gamelift = GameLift::new();
    let first = create_fleet(&gamelift, ec2_fleet("lookup-one")).await;
    let second = create_fleet(&gamelift, ec2_fleet("lookup-two")).await;
    wait_for_due_transitions();

    let first_arn = gamelift_arn(&format!("fleet/{first}"));
    // Well-formed against the modelled `FleetIdOrArn` pattern
    // (`^[a-z]*fleet-[a-zA-Z0-9\-]+$|^arn:.*:[a-z]*fleet\/[a-z]*fleet-[a-zA-Z0-9\-]+$`),
    // and matching no stored fleet.
    let absent = "fleet-does-not-exist".to_string();
    let absent_uuid = "fleet-00000000-0000-0000-0000-000000000000".to_string();
    let absent_arn = gamelift_arn("fleet/fleet-nope");

    // Rows differ along the count of identifiers, their form (id or ARN), and how many
    // of them are stored; the expectation is always "the stored ones, once each".
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        ("one unknown id", vec![absent.clone()], vec![]),
        ("one unknown UUID-shaped id", vec![absent_uuid.clone()], vec![]),
        ("one unknown ARN", vec![absent_arn.clone()], vec![]),
        (
            "three unknown identifiers of mixed form",
            vec![absent.clone(), absent_uuid.clone(), absent_arn.clone()],
            vec![],
        ),
        (
            "the same unknown id twice",
            vec![absent.clone(), absent.clone()],
            vec![],
        ),
        (
            "an unknown id before a stored one",
            vec![absent.clone(), first.clone()],
            vec![first.clone()],
        ),
        (
            "an unknown id after a stored one",
            vec![first.clone(), absent.clone()],
            vec![first.clone()],
        ),
        (
            "both stored fleets around two unknown ids",
            vec![
                first.clone(),
                absent.clone(),
                second.clone(),
                absent_uuid.clone(),
            ],
            vec![first.clone(), second.clone()],
        ),
        (
            "an unknown ARN beside the stored fleet's ARN",
            vec![absent_arn.clone(), first_arn.clone()],
            vec![first.clone()],
        ),
    ];

    for (label, fleet_ids, expected) in &rows {
        let (status, body) = gamelift
            .call("DescribeFleetAttributes", &json!({"FleetIds": fleet_ids}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: an unknown fleet identifier must not fail the request, \
             got HTTP {status} with body {body}"
        );
        let mut reported = reported_fleet_ids(&body);
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {label:?}: expected the stored fleets {wanted:?}, got {reported:?} in {body}"
        );
        assert!(
            body["FleetAttributes"].is_array(),
            "case {label:?}: FleetAttributes must be present as a list, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {label:?}: a whole result set carries no NextToken, got {body}"
        );
    }

    // `Limit` is "ignored when the request specifies one or a list of fleet IDs", so an
    // all-unknown list is an empty page there too, not a token for a next one.
    let (status, body) = gamelift
        .call(
            "DescribeFleetAttributes",
            &json!({"FleetIds": [&absent], "Limit": 1}),
        )
        .await;
    assert_eq!(
        (status, body["FleetAttributes"].clone()),
        (200, json!([])),
        "case unknown id with Limit: expected 200 with an empty page, got {status} {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case unknown id with Limit: an empty page carries no NextToken, got {body}"
    );

    // None of those reads moved anything: both stored fleets still read back ACTIVE.
    let described = gamelift
        .ok(
            "DescribeFleetAttributes",
            json!({"FleetIds": [&first, &second]}),
        )
        .await;
    let statuses: Vec<&str> = described["FleetAttributes"]
        .as_array()
        .expect("FleetAttributes must be a list")
        .iter()
        .map(|attribute| attribute["Status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        statuses,
        vec!["ACTIVE", "ACTIVE"],
        "the stored fleets must survive the unknown-identifier reads, got {described}"
    );
}

/// `DescribeGameSessions` reports the stored sessions its selector picks out; a selector
/// that picks out none — an unknown fleet, an unknown session id, a location the fleet
/// does not deploy to — is an empty page with status 200, while the selectors that do
/// pick out the stored session still report it.
#[tokio::test]
async fn describing_sessions_of_an_unknown_selector_reports_an_empty_page() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("session-lookup")).await;
    wait_for_due_transitions();
    let created = gamelift
        .ok(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 4}),
        )
        .await;
    let game_session_id = created["GameSession"]["GameSessionId"]
        .as_str()
        .expect("CreateGameSession must report a GameSessionId")
        .to_string();
    wait_for_due_transitions();

    let fleet_arn = gamelift_arn(&format!("fleet/{fleet_id}"));
    // Each row is a selector, and how many of the one stored session it picks out. The
    // identifiers are well formed against the modelled patterns (`FleetIdOrArn`,
    // `ArnStringModel` `^[a-zA-Z0-9:/-]+$`) and name nothing stored.
    let rows: Vec<(&str, Value, usize)> = vec![
        ("an unknown fleet id", json!({"FleetId": "fleet-absent"}), 0),
        (
            "an unknown fleet UUID-shaped id",
            json!({"FleetId": "fleet-00000000-0000-0000-0000-000000000000"}),
            0,
        ),
        (
            "an unknown fleet ARN",
            json!({"FleetId": gamelift_arn("fleet/fleet-absent")}),
            0,
        ),
        (
            "an unknown game session on an unknown fleet",
            json!({
                "GameSessionId":
                    gamelift_arn("gamesession/fleet-absent/absent-session"),
            }),
            0,
        ),
        (
            "an unknown game session on the stored fleet",
            json!({
                "GameSessionId":
                    gamelift_arn(&format!("gamesession/{fleet_id}/absent-session")),
            }),
            0,
        ),
        (
            "an unknown game session in the remote-location ARN form",
            json!({
                "GameSessionId":
                    gamelift_arn(&format!("gamesession/{fleet_id}/eu-west-1/absent")),
            }),
            0,
        ),
        (
            "the stored fleet, narrowed to a location it does not deploy to",
            json!({"FleetId": &fleet_id, "Location": "ap-south-1"}),
            0,
        ),
        (
            "an unknown fleet, narrowed to the stored fleet's own location",
            json!({"FleetId": "fleet-absent", "Location": REGION}),
            0,
        ),
        (
            "an unknown fleet with a status filter",
            json!({"FleetId": "fleet-absent", "StatusFilter": "ACTIVE"}),
            0,
        ),
        // The control rows: a selector that DOES pick the session out still reports it,
        // so the empty pages above are not a blanket empty answer.
        ("the stored fleet by id", json!({"FleetId": &fleet_id}), 1),
        ("the stored fleet by ARN", json!({"FleetId": &fleet_arn}), 1),
        (
            "the stored game session by id",
            json!({"GameSessionId": &game_session_id}),
            1,
        ),
    ];

    for (label, request, expected) in &rows {
        let (status, body) = gamelift.call("DescribeGameSessions", request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a selector that matches nothing must not fail the request, \
             got HTTP {status} with body {body}"
        );
        let reported = body["GameSessions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_else(|| panic!("case {label:?}: GameSessions must be a list, got {body}"));
        assert_eq!(
            reported, *expected,
            "case {label:?}: expected {expected} game session(s), got {reported} in {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {label:?}: a whole result set carries no NextToken, got {body}"
        );
    }
}

/// A `NotFoundException` message is built from the request: the identifier the caller
/// supplied, verbatim, in the template the operation uses — one template per operation,
/// never the stored or resolved value and never one shared wording.
#[tokio::test]
async fn not_found_messages_are_built_from_the_request() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("message-templates")).await;
    wait_for_due_transitions();

    struct Case {
        label: String,
        operation: &'static str,
        request: Value,
        expected: String,
    }

    let mut cases = Vec::new();
    // Fleet identifiers that are well formed and name no stored fleet, differing in
    // form (bare id, UUID-shaped id, ARN) and length.
    for fleet in [
        "fleet-absent".to_string(),
        "fleet-00000000-0000-0000-0000-000000000000".to_string(),
        gamelift_arn("fleet/fleet-absent-by-arn"),
    ] {
        cases.push(Case {
            label: format!("DeleteFleet on {fleet}"),
            operation: "DeleteFleet",
            request: json!({"FleetId": &fleet}),
            expected: format!("Unable to find fleet '{fleet}'."),
        });
        cases.push(Case {
            label: format!("CreateGameSession on {fleet}"),
            operation: "CreateGameSession",
            request: json!({"FleetId": &fleet, "MaximumPlayerSessionCount": 2}),
            expected: format!("Cannot find fleet with ID {fleet}"),
        });
    }
    // Game session identifiers in both documented ARN forms (home Region and remote
    // location), on a stored fleet and on an absent one.
    for session in [
        gamelift_arn(&format!("gamesession/{fleet_id}/absent-session")),
        gamelift_arn(&format!("gamesession/{fleet_id}/eu-west-1/absent-session")),
        gamelift_arn("gamesession/fleet-absent/absent-session"),
    ] {
        cases.push(Case {
            label: format!("CreatePlayerSession in {session}"),
            operation: "CreatePlayerSession",
            request: json!({"GameSessionId": &session, "PlayerId": "player-1"}),
            expected: format!("GameSession {session} does not exist"),
        });
    }

    for case in &cases {
        let (status, body) = gamelift.call(case.operation, &case.request).await;
        let code = crate::hosting_loop::error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {}: expected 400 NotFoundException, got {status} {code} with body {body}",
            case.label
        );
        let message = error_message(&body);
        assert_eq!(
            message, case.expected,
            "case {}: expected Message {:?}, got {:?}",
            case.label, case.expected, message
        );
    }

    // The fleet that IS there is untouched: none of the refusals above deleted it.
    let described = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
        .await;
    assert_eq!(
        described["FleetAttributes"][0]["Status"],
        json!("ACTIVE"),
        "the stored fleet must survive the refused requests, got {described}"
    );
}
