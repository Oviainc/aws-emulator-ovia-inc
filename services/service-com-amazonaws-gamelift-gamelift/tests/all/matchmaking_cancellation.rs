// SPDX-License-Identifier: CC0-1.0

//! Cancelling a matchmaking request over the wire: `StopMatchmaking` stops work on the
//! ticket a caller names, so the stored ticket moves to the modelled `CANCELLED` status
//! and carries the instant it stopped being processed — whatever form its id takes,
//! whichever in-process status it was in, and whatever the request it carries.
//!
//! A ticket id no ticket carries is refused, a request the service has already finished
//! with cannot be stopped, and a cancelled ticket is out of the clock's progression: it is
//! neither picked up nor timed out afterwards.
//!
//! The harness (`GameLift`, `wait_for_due_transitions`) is the one `hosting_loop` already
//! drives the production dispatch path with; the matchmaker helpers are `matchmaking`'s.

use std::time::Duration;

use serde_json::{json, Value};

use crate::hosting_loop::{error_code, error_message, wait_for_due_transitions, GameLift};
use crate::matchmaking::matchmaker;

/// One matchmaking request to cancel.
struct Case {
    label: &'static str,
    /// The `TicketId` the request names, or `None` to let the service mint one — "if no
    /// ticket ID is specified here, Amazon GameLift Servers will generate one in the form
    /// of a UUID".
    ticket_id: Option<String>,
    players: Value,
    /// The in-process status the ticket is stopped in: `QUEUED` straight after the start,
    /// `SEARCHING` once the service has picked it up.
    stop_while: &'static str,
}

/// Start `case`'s request on `configuration` and return the ticket it reports.
async fn start(gamelift: &GameLift, configuration: &str, case: &Case) -> Value {
    let mut request = json!({"ConfigurationName": configuration, "Players": &case.players});
    if let Some(ticket_id) = &case.ticket_id {
        request["TicketId"] = json!(ticket_id);
    }
    let started = gamelift.ok("StartMatchmaking", request).await;
    let ticket = started["MatchmakingTicket"].clone();
    assert_eq!(
        ticket["Status"],
        json!("QUEUED"),
        "case {:?}: a new ticket must be QUEUED, got {ticket}",
        case.label
    );
    ticket
}

/// Cancel the ticket `started` names and assert the service reports it cancelled: the
/// stop answers an empty struct, and the ticket reads back `CANCELLED`, stamped with the
/// instant it stopped and otherwise unchanged. Answers the cancelled ticket.
async fn cancel(gamelift: &GameLift, label: &str, started: &Value) -> Value {
    let ticket_id = started["TicketId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the ticket must carry an id, got {started}"))
        .to_string();

    // "If the operation is successful, the service sends back an empty JSON struct with the
    // HTTP 200 response (not an empty HTTP body)."
    let stopped = gamelift
        .ok("StopMatchmaking", json!({"TicketId": &ticket_id}))
        .await;
    assert_eq!(
        stopped,
        json!({}),
        "case {label:?}: a successful stop must answer an empty JSON struct, got {stopped}"
    );

    // "If successful, work on the ticket is stopped, and the ticket status is changed to
    // CANCELLED": the stored ticket is what every later read serves.
    let described = gamelift
        .ok("DescribeMatchmaking", json!({"TicketIds": [&ticket_id]}))
        .await;
    let ticket = described["TicketList"][0].clone();
    assert_eq!(
        ticket["Status"],
        json!("CANCELLED"),
        "case {label:?}: a stopped ticket must be CANCELLED, got {ticket}"
    );
    // "Time stamp indicating when the matchmaking request stopped being processed due to
    // successful completion, timeout, or cancellation."
    let end_time = ticket["EndTime"].as_f64().unwrap_or_else(|| {
        panic!("case {label:?}: a cancelled ticket must carry an EndTime, got {ticket}")
    });
    let start_time = ticket["StartTime"].as_f64().unwrap_or_else(|| {
        panic!("case {label:?}: a ticket must carry a StartTime, got {ticket}")
    });
    assert!(
        end_time >= start_time,
        "case {label:?}: the request stopped being processed no earlier than it started, \
         got EndTime {end_time} and StartTime {start_time}"
    );
    // The cancellation changed the status and the end time, and nothing the request
    // carried: the ticket still names the matchmaker it was started on and the players it
    // was started for.
    for member in [
        "TicketId",
        "ConfigurationName",
        "ConfigurationArn",
        "StartTime",
        "Players",
    ] {
        assert_eq!(
            ticket[member], started[member],
            "case {label:?}: {member} must be unchanged by the cancellation, got {ticket}"
        );
    }
    // `StatusReason` is the "code to explain the current status", which the service fills
    // for a request that did not complete on its own; a request the caller stopped carries
    // none, and no match was made for it, so it reports no connection information either.
    assert!(
        ticket["StatusReason"].is_null(),
        "case {label:?}: a ticket the caller stopped carries no StatusReason, got {ticket}"
    );
    assert!(
        ticket["GameSessionConnectionInfo"].is_null(),
        "case {label:?}: a cancelled ticket matched nothing, so it reports no game session \
         connection information, got {ticket}"
    );
    ticket
}

/// Every matchmaking request that is still being processed is cancelled by
/// `StopMatchmaking`, and reads back `CANCELLED` through `DescribeMatchmaking`.
///
/// The rows differ along what the rule generalises over: the form of the ticket id (a
/// caller-chosen one, one carrying the dots the modelled `MatchmakingIdStringModel`
/// pattern `^[a-zA-Z0-9-\.]*$` allows, one of the modelled maximum length 128, and one the
/// service mints), the number of players the request carries and what each brings, and the
/// in-process status the ticket is stopped in.
#[tokio::test]
async fn a_request_being_processed_is_cancelled_in_every_form_it_takes() {
    let gamelift = GameLift::new();
    let configuration = matchmaker(&gamelift, "cancel-matchmaker", 43200).await;

    let cases = vec![
        Case {
            label: "one player, a caller-chosen ticket id, stopped while QUEUED",
            ticket_id: Some("cancel-one-player".to_string()),
            players: json!([{"PlayerId": "player-1"}]),
            stop_while: "QUEUED",
        },
        Case {
            label: "a ticket id carrying the dots the modelled pattern allows",
            ticket_id: Some("cancel.dotted.id-2".to_string()),
            players: json!([{"PlayerId": "player-2"}]),
            stop_while: "QUEUED",
        },
        Case {
            // `MatchmakingIdStringModel` is len 0..128, so this id is at the bound.
            label: "a ticket id of the modelled maximum length",
            ticket_id: Some("c".repeat(128)),
            players: json!([{"PlayerId": "player-3"}]),
            stop_while: "QUEUED",
        },
        Case {
            label: "three players with attributes and latencies, stopped while QUEUED",
            ticket_id: Some("cancel-party".to_string()),
            players: json!([
                {
                    "PlayerId": "player-4",
                    "PlayerAttributes": {"skill": {"N": 42.5}, "role": {"S": "medic"}},
                    "LatencyInMs": {"us-west-2": 30},
                },
                {"PlayerId": "player-5", "Team": "red"},
                {"PlayerId": "player-6", "Team": "blue"},
            ]),
            stop_while: "QUEUED",
        },
        Case {
            label: "no ticket id at all, which the service mints, stopped while SEARCHING",
            ticket_id: None,
            players: json!([{"PlayerId": "player-7"}]),
            stop_while: "SEARCHING",
        },
        Case {
            label: "a caller-chosen ticket id, stopped while SEARCHING",
            ticket_id: Some("cancel-searching".to_string()),
            players: json!([{"PlayerId": "player-8"}, {"PlayerId": "player-9"}]),
            stop_while: "SEARCHING",
        },
    ];

    // Every request is started first, so the one wait below carries every row that must be
    // stopped past the pick-up to its `SEARCHING` status.
    let mut started: Vec<(&Case, Value)> = Vec::new();
    for case in &cases {
        let ticket = start(&gamelift, &configuration, case).await;
        started.push((case, ticket));
    }

    for (case, ticket) in &started {
        if case.stop_while == "QUEUED" {
            cancel(&gamelift, case.label, ticket).await;
        }
    }

    wait_for_due_transitions();
    for (case, ticket) in &started {
        if case.stop_while != "SEARCHING" {
            continue;
        }
        let ticket_id = ticket["TicketId"].as_str().unwrap_or_default().to_string();
        let described = gamelift
            .ok("DescribeMatchmaking", json!({"TicketIds": [&ticket_id]}))
            .await;
        assert_eq!(
            described["TicketList"][0]["Status"],
            json!("SEARCHING"),
            "case {:?}: a picked-up ticket must be SEARCHING before it is stopped, got {described}",
            case.label
        );
        cancel(&gamelift, case.label, ticket).await;
    }

    // Cancelling those requests left every OTHER ticket where it was: the rows stopped
    // while QUEUED are still CANCELLED, and the pick-up the clock owed them never moved
    // them back.
    let ids: Vec<String> = started
        .iter()
        .map(|(_, ticket)| ticket["TicketId"].as_str().unwrap_or_default().to_string())
        .collect();
    let described = gamelift
        .ok("DescribeMatchmaking", json!({"TicketIds": &ids}))
        .await;
    let statuses: Vec<Value> = described["TicketList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|ticket| ticket["Status"].clone())
        .collect();
    assert_eq!(
        statuses,
        vec![json!("CANCELLED"); cases.len()],
        "every stopped ticket must stay CANCELLED, got {described}"
    );
}

/// A cancelled request is out of the progression the clock drives: past the instants its
/// pick-up and its request timeout were due on, it is still `CANCELLED` and still carries
/// the instant the caller stopped it.
#[tokio::test]
async fn a_cancelled_request_is_neither_picked_up_nor_timed_out() {
    let gamelift = GameLift::new();
    // The rows differ along the matchmaker's request timeout: one that cannot elapse
    // during the test, and the shortest the model allows
    // (`MatchmakingRequestTimeoutInteger`, range 1..43200), which elapses while the test
    // waits.
    let patient = matchmaker(&gamelift, "patient-cancel-matchmaker", 43200).await;
    let impatient = matchmaker(&gamelift, "impatient-cancel-matchmaker", 1).await;

    let mut cancelled: Vec<(&str, String, Value)> = Vec::new();
    for (label, configuration, ticket_id) in [
        (
            "a matchmaker whose request timeout cannot elapse",
            &patient,
            "cancel-patient",
        ),
        (
            "a matchmaker with a one-second request timeout",
            &impatient,
            "cancel-impatient",
        ),
    ] {
        let case = Case {
            label,
            ticket_id: Some(ticket_id.to_string()),
            players: json!([{"PlayerId": "player-1"}]),
            stop_while: "QUEUED",
        };
        let started = start(&gamelift, configuration, &case).await;
        let ticket = cancel(&gamelift, label, &started).await;
        cancelled.push((label, ticket_id.to_string(), ticket));
    }

    // Past the pick-up span AND past the one-second request timeout.
    wait_for_due_transitions();
    std::thread::sleep(Duration::from_millis(1_100));
    for (label, ticket_id, before) in &cancelled {
        let described = gamelift
            .ok("DescribeMatchmaking", json!({"TicketIds": [ticket_id]}))
            .await;
        let after = described["TicketList"][0].clone();
        assert_eq!(
            after, *before,
            "case {label:?}: a cancelled ticket must be unchanged by the clock, got {after}"
        );
    }
}

/// `StopMatchmaking` refuses a ticket id no ticket carries, and a request the service has
/// already finished with: there is no work left on it to stop.
///
/// The rows differ along why the request cannot be served: the ticket does not exist (in
/// each form an absent id takes), or it exists in a terminal status — one the caller
/// cancelled, and one that timed out on its own.
#[tokio::test]
async fn a_request_that_is_not_being_processed_is_refused() {
    let gamelift = GameLift::new();
    let patient = matchmaker(&gamelift, "refusal-matchmaker", 43200).await;
    let impatient = matchmaker(&gamelift, "refusal-impatient-matchmaker", 1).await;

    // A ticket id that names nothing is the modelled `NotFoundException` — "the requested
    // resource was not found[;] the resource was either not created yet or deleted" — and
    // the refusal states which id was asked for.
    for (label, ticket_id) in [
        ("an id no ticket was ever started with", "cancel-absent"),
        ("the empty id the modelled pattern admits", ""),
        ("an id in the UUID form the service mints", "11111111-2222-3333-4444-555555555555"),
    ] {
        let (status, body) = gamelift
            .call("StopMatchmaking", &json!({"TicketId": ticket_id}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an absent ticket must be refused with NotFoundException, got \
             {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Matchmaking ticket not found: {ticket_id}"),
            "case {label:?}: the refusal must state which ticket was asked for, got {body}"
        );
    }

    // A ticket in a terminal status: one the caller already cancelled, and one the clock
    // timed out.
    gamelift
        .ok(
            "StartMatchmaking",
            json!({
                "TicketId": "refusal-cancelled",
                "ConfigurationName": &patient,
                "Players": [{"PlayerId": "player-1"}],
            }),
        )
        .await;
    gamelift
        .ok("StopMatchmaking", json!({"TicketId": "refusal-cancelled"}))
        .await;
    gamelift
        .ok(
            "StartMatchmaking",
            json!({
                "TicketId": "refusal-timed-out",
                "ConfigurationName": &impatient,
                "Players": [{"PlayerId": "player-2"}],
            }),
        )
        .await;
    std::thread::sleep(Duration::from_millis(1_100));

    for (label, ticket_id, expected_status) in [
        (
            "a request the caller already cancelled",
            "refusal-cancelled",
            "CANCELLED",
        ),
        (
            "a request that timed out on its own",
            "refusal-timed-out",
            "TIMED_OUT",
        ),
    ] {
        let described = gamelift
            .ok("DescribeMatchmaking", json!({"TicketIds": [ticket_id]}))
            .await;
        let before = described["TicketList"][0].clone();
        assert_eq!(
            before["Status"],
            json!(expected_status),
            "case {label:?}: the ticket must be {expected_status} before the stop, got {before}"
        );

        let (status, body) = gamelift
            .call("StopMatchmaking", &json!({"TicketId": ticket_id}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a request that is not being processed must be refused, got \
             {status} {body}"
        );
        assert!(
            error_message(&body).contains(expected_status),
            "case {label:?}: the refusal must state the status the ticket is in, got {body}"
        );

        // The refused stop changed nothing: the ticket keeps the status it ended in.
        let described = gamelift
            .ok("DescribeMatchmaking", json!({"TicketIds": [ticket_id]}))
            .await;
        assert_eq!(
            described["TicketList"][0], before,
            "case {label:?}: a refused stop must leave the ticket as it was, got {described}"
        );
    }
}
