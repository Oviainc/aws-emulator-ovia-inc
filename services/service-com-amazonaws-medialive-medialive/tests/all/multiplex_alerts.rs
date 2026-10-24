// SPDX-License-Identifier: CC0-1.0

//! `ListMultiplexAlerts`: which alerts a multiplex has, when the service raises
//! one and when it clears it, how the listing filters and pages them, and what it
//! refuses — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES spanning what each rule generalises over: how many
//! programs the multiplex holds and what feeds each of them, the lifecycle state
//! the multiplex is in, the `StateFilter` asked for and the page size asked for.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    multiplex_alert_state, multiplex_alert_type, MAX_LIST_PAGE_SIZE,
    MULTIPLEX_ALERT_STATE_FILTERS, MULTIPLEX_ALERT_STATE_FILTER_ALL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{describe_multiplex, settings, MULTIPLEXES};
use crate::multiplex_programs::{create_program, program_settings};
use crate::multiplex_start::{multiplex_with_settings, start_multiplex};
use crate::multiplex_stop::stop_multiplex;
use crate::support::{epoch_seconds, timestamp};
use crate::wire::{call, error_code, error_message, medialive};

/// The one alert type this service raises on a multiplex, read from the state
/// slice that names it rather than spelled again here, so the test and the
/// service cannot disagree about it.
const NO_RUNNING_CHANNEL: &str = multiplex_alert_type::PROGRAM_NO_RUNNING_CHANNEL;

/// `ListMultiplexAlerts` on one multiplex, with `query` appended verbatim
/// (`smithy_query(query="operation", operation="ListMultiplexAlerts")`: GET
/// `/prod/multiplexes/{MultiplexId}/alerts`, where `MaxResults`, `NextToken` and
/// `StateFilter` are all `@httpQuery`).
async fn list(router: &MultiServiceRouter, multiplex_id: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("{MULTIPLEXES}/{multiplex_id}/alerts{query}"),
        None,
    )
    .await
}

/// The `alerts` member as a list, whatever its length
/// (`ListMultiplexAlertsResponse$Alerts`, jsonName `alerts`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["alerts"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an alerts list, got {body}"))
}

/// One page of alerts, asserted to have answered 200.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, body) = list(router, multiplex_id, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListMultiplexAlerts{query} to answer 200, got {status}: {body}"
    );
    let alerts = listed(case, &body);
    let token = body["nextToken"].as_str().map(ToString::to_string);
    if token.is_some() {
        assert!(
            !alerts.is_empty(),
            "case {case:?}: expected no nextToken beside an empty page, got {body}"
        );
    }
    (alerts, token)
}

/// Every alert the listing reports, walked page by page, and the size of each
/// page.
async fn every_alert(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    state_filter: Option<&str>,
    page_size: Option<i32>,
) -> (Vec<Value>, Vec<usize>) {
    let mut alerts: Vec<Value> = Vec::new();
    let mut pages: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = String::new();
        for parameter in [
            state_filter.map(|filter| format!("stateFilter={filter}")),
            page_size.map(|size| format!("maxResults={size}")),
            token.as_ref().map(|next| format!("nextToken={next}")),
        ]
        .into_iter()
        .flatten()
        {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&parameter);
        }

        let (reported, next) = page(router, case, multiplex_id, &query).await;
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            reported.len() <= allowed,
            "case {case:?}: expected at most {allowed} alerts in a page, got {}",
            reported.len()
        );
        pages.push(reported.len());
        alerts.extend(reported);
        token = next;
        if token.is_none() {
            break;
        }
        assert!(
            pages.len() <= 256,
            "case {case:?}: expected pagination to terminate, got pages {pages:?}"
        );
    }
    (alerts, pages)
}

/// A program created in `multiplex_id` under `program_name`, asserted to have
/// succeeded. `program_number` is "Unique program number" within the multiplex.
async fn program(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    program_name: &str,
    program_number: i64,
) {
    let (status, created) = create_program(
        router,
        multiplex_id,
        program_name,
        &format!("token-{multiplex_id}-{program_name}"),
        program_settings(program_number),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplexProgram({program_name}): expected 201, got {status}: \
         {created}"
    );
}

/// A channel whose one output destination names `multiplex_id` and
/// `program_name` (`OutputDestination$MultiplexSettings` is a
/// `MultiplexProgramChannelDestinationSettings` carrying exactly that pair), as
/// its minted id.
async fn feeding_channel(
    router: &MultiServiceRouter,
    case: &str,
    multiplex_id: &str,
    program_name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": format!("feeds-{program_name}"),
            "destinations": [{
                "id": "multiplex-out",
                "multiplexSettings": {
                    "multiplexId": multiplex_id,
                    "programName": program_name,
                },
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel({program_name}): expected 201, got {status}: {created}"
    );
    created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Start the channel `channel_id` (`POST /prod/channels/{ChannelId}/start`).
async fn start_channel(router: &MultiServiceRouter, case: &str, channel_id: &str) {
    let (status, started) = call(
        router,
        "POST",
        &format!("/prod/channels/{channel_id}/start"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartChannel({channel_id}): expected 200, got {status}: {started}"
    );
}

/// Stop the channel `channel_id` (`POST /prod/channels/{ChannelId}/stop`).
async fn stop_channel(router: &MultiServiceRouter, case: &str, channel_id: &str) {
    let (status, stopped) = call(
        router,
        "POST",
        &format!("/prod/channels/{channel_id}/stop"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StopChannel({channel_id}): expected 200, got {status}: {stopped}"
    );
}

/// Start `multiplex_id` and let the clock settle it into `RUNNING`, asserting it
/// got there.
async fn run_multiplex(router: &MultiServiceRouter, case: &str, multiplex_id: &str) {
    let (status, starting) = start_multiplex(router, multiplex_id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartMultiplex: expected 202, got {status}: {starting}"
    );
    wait_for_the_clock();
    let (status, running) = describe_multiplex(router, multiplex_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {running}"
    );
    assert_eq!(
        running["state"],
        json!("RUNNING"),
        "case {case:?}: the test did not reach RUNNING, it reached {:?}",
        running["state"]
    );
}

/// The alert's `State` member, named in the failure so a wrong state is readable.
fn alert_state(case: &str, alert: &Value) -> String {
    alert["state"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an alert state, got {alert}"))
        .to_string()
}

/// The alert's `Id` member, which is also the `NextToken` a page resumes at.
fn alert_id(case: &str, alert: &Value) -> String {
    let id = alert["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an alert id, got {alert}"));
    assert!(
        id.parse::<u64>().is_ok(),
        "case {case:?}: expected the alert id to be the token form the listing resumes from, \
         got {id:?}"
    );
    id.to_string()
}

/// Every member of a `MultiplexAlert` asserted on a SET alert about
/// `program_name`: the type, the state, the message naming the program, an
/// `Id`, a `SetTimestamp` and NO `ClearedTimestamp`.
fn assert_set_alert(case: &str, alert: &Value, program_name: &str) {
    assert_eq!(
        alert["alertType"],
        json!(NO_RUNNING_CHANNEL),
        "case {case:?}: expected the alertType the service raises, got {:?}",
        alert["alertType"]
    );
    assert_eq!(
        alert_state(case, alert),
        multiplex_alert_state::SET,
        "case {case:?}: expected a SET alert, got {alert}"
    );
    let message = alert["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(program_name),
        "case {case:?}: expected the message to name the program {program_name:?}, got \
         {message:?}"
    );
    alert_id(case, alert);
    timestamp(case, "setTimestamp", alert);
    assert_eq!(
        alert["clearedTimestamp"],
        Value::Null,
        "case {case:?}: expected no clearedTimestamp on a SET alert, got {alert}"
    );
}

/// What feeds one program of the multiplex under test — the dimension the
/// raise rule generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feed {
    /// No channel's output destination names the program at all.
    Nothing,
    /// A channel names it, but that channel is not running, so none of its
    /// pipelines is healthy.
    StoppedChannel,
    /// A channel names it and is RUNNING, so its output reaches the program.
    RunningChannel,
}

/// One row of the raise table: the multiplex's programs and what feeds each.
struct Programs {
    case: &'static str,
    /// (program name, what feeds it). The names are not prefixes of one another,
    /// so "the message names this program" is unambiguous.
    programs: &'static [(&'static str, Feed)],
}

/// Rule: while a multiplex is transmitting, the service raises one SET alert per
/// program of it whose output no running MediaLive channel is feeding, and raises
/// none for a program a running channel feeds; each alert names the program it is
/// about, carries the instant it was set and carries no cleared instant while it
/// is still happening.
///
/// The rows differ along what the rule generalises over: how many programs the
/// multiplex holds, and what feeds each of them — nothing at all, a channel that
/// is not running, or a running channel — in every combination from none alerting
/// to all of them alerting.
#[tokio::test]
async fn a_running_multiplex_alerts_on_every_program_no_running_channel_feeds() {
    let table = [
        Programs {
            case: "no programs at all",
            programs: &[],
        },
        Programs {
            case: "one program, nothing feeds it",
            programs: &[("alpha", Feed::Nothing)],
        },
        Programs {
            case: "one program, a running channel feeds it",
            programs: &[("alpha", Feed::RunningChannel)],
        },
        Programs {
            case: "one program, a stopped channel names it",
            programs: &[("alpha", Feed::StoppedChannel)],
        },
        Programs {
            case: "three programs, one of each",
            programs: &[
                ("alpha", Feed::Nothing),
                ("beta", Feed::StoppedChannel),
                ("gamma", Feed::RunningChannel),
            ],
        },
        Programs {
            case: "four programs, two fed",
            programs: &[
                ("delta", Feed::RunningChannel),
                ("epsilon", Feed::Nothing),
                ("zeta", Feed::RunningChannel),
                ("eta", Feed::Nothing),
            ],
        },
    ];

    for Programs { case, programs } in table {
        let router = medialive();
        let multiplex_id = multiplex_with_settings(&router, case, "alerting", settings(2_000_000, 1)).await;
        for (index, (program_name, feed)) in programs.iter().enumerate() {
            program(&router, case, &multiplex_id, program_name, index as i64 + 1).await;
            if *feed != Feed::Nothing {
                feeding_channel(&router, case, &multiplex_id, program_name).await;
            }
        }

        // The create transitions of the multiplex and of every channel come due
        // first, so each channel is stopped (IDLE) rather than still creating.
        wait_for_the_clock();
        for (program_name, feed) in programs {
            if *feed == Feed::RunningChannel {
                let channel_id = call(
                    &router,
                    "GET",
                    &format!("{MULTIPLEXES}/{multiplex_id}/programs/{program_name}"),
                    None,
                )
                .await
                .1["channelId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                start_channel(&router, case, &channel_id).await;
            }
        }
        // ... and the channel starts settle before the multiplex runs, so a
        // program a running channel feeds is fed from the first instant the
        // multiplex transmits.
        wait_for_the_clock();
        run_multiplex(&router, case, &multiplex_id).await;

        let expected: Vec<&str> = programs
            .iter()
            .filter(|(_, feed)| *feed != Feed::RunningChannel)
            .map(|(program_name, _)| *program_name)
            .collect();
        // Alerts are reported in the order they were raised, and the programs of
        // one multiplex are walked in name order, so the expectation is in name
        // order too.
        let mut expected_in_order = expected.clone();
        expected_in_order.sort_unstable();

        let (alerts, _) = every_alert(
            &router,
            case,
            &multiplex_id,
            Some(multiplex_alert_state::SET),
            None,
        )
        .await;
        assert_eq!(
            alerts.len(),
            expected_in_order.len(),
            "case {case:?}: expected one SET alert per unfed program {expected_in_order:?}, \
             got {alerts:?}"
        );
        for (alert, program_name) in alerts.iter().zip(&expected_in_order) {
            assert_set_alert(case, alert, program_name);
        }
        // No alert is about a program a running channel feeds.
        for (program_name, feed) in programs {
            if *feed != Feed::RunningChannel {
                continue;
            }
            assert!(
                !alerts.iter().any(|alert| alert["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(program_name)),
                "case {case:?}: expected no alert about the fed program {program_name:?}, \
                 got {alerts:?}"
            );
        }
    }
}

/// Rule: an alert is raised only while the multiplex is transmitting — in every
/// state in which none of its pipelines is up there is nothing to be wrong, so a
/// multiplex that has not run has no alert at all and stopping one clears the
/// alerts its run raised.
///
/// The rows span the lifecycle states this service can put a multiplex in, in the
/// order a multiplex passes through them, because that is what the rule
/// generalises over.
#[tokio::test]
async fn alerts_are_raised_only_while_the_multiplex_is_running() {
    // (case, the state the multiplex is in when the listing runs, the alerts
    // expected there: how many, and in which state)
    let table: [(&str, &str, usize, &str); 5] = [
        ("still creating", "CREATING", 0, multiplex_alert_state::SET),
        ("stopped", "IDLE", 0, multiplex_alert_state::SET),
        ("starting", "STARTING", 0, multiplex_alert_state::SET),
        ("running", "RUNNING", 2, multiplex_alert_state::SET),
        ("stopped again", "IDLE", 2, multiplex_alert_state::CLEARED),
    ];

    for (case, state, alerts_expected, alert_state_expected) in table {
        let router = medialive();
        let multiplex_id = multiplex_with_settings(&router, case, "lifecycle", settings(2_000_000, 1)).await;
        // Two programs, neither fed by anything, so a transmitting multiplex has
        // exactly two conditions holding.
        for (index, program_name) in ["alpha", "beta"].into_iter().enumerate() {
            program(&router, case, &multiplex_id, program_name, index as i64 + 1).await;
        }

        if state != "CREATING" {
            wait_for_the_clock();
        }
        if state != "CREATING" && state != "IDLE" || case == "stopped again" {
            let (status, starting) = start_multiplex(&router, &multiplex_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StartMultiplex: expected 202, got {status}: {starting}"
            );
        }
        if state == "RUNNING" || case == "stopped again" {
            wait_for_the_clock();
        }
        if case == "stopped again" {
            // The run has to have RAISED the alerts before the stop can clear
            // them, which is what this listing lets the service observe.
            let (raised, _) = every_alert(&router, case, &multiplex_id, None, None).await;
            assert_eq!(
                raised.len(),
                2,
                "case {case:?}: expected the run to have raised two alerts, got {raised:?}"
            );
            let (status, stopped) = stop_multiplex(&router, &multiplex_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StopMultiplex: expected 202, got {status}: {stopped}"
            );
            wait_for_the_clock();
        }

        let (status, observed) = describe_multiplex(&router, &multiplex_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {observed}"
        );
        assert_eq!(
            observed["state"],
            json!(state),
            "case {case:?}: the test did not reach {state}, it reached {:?}",
            observed["state"]
        );

        let (alerts, pages) = every_alert(&router, case, &multiplex_id, None, None).await;
        assert_eq!(
            alerts.len(),
            alerts_expected,
            "case {case:?}: expected {alerts_expected} alerts on a {state} multiplex, got \
             {alerts:?}"
        );
        assert_eq!(
            pages,
            vec![alerts_expected],
            "case {case:?}: expected one page, got {pages:?}"
        );
        for alert in &alerts {
            assert_eq!(
                alert_state(case, alert),
                alert_state_expected,
                "case {case:?}: expected a {alert_state_expected} alert on a {state} multiplex, \
                 got {alert}"
            );
        }
    }
}

/// Rule: an alert instance is one OCCURRENCE of its condition — it stays SET with
/// the id and the set instant it was raised with while the condition holds, it is
/// CLEARED (keeping both, and gaining the instant it was cleared at) when the
/// condition stops holding, and a condition that holds again raises a NEW
/// instance beside the cleared one rather than reviving it.
#[tokio::test]
async fn an_alert_clears_when_its_condition_stops_and_a_later_occurrence_is_a_new_alert() {
    let case = "a program whose channel starts and stops again";
    let router = medialive();
    let multiplex_id = multiplex_with_settings(&router, case, "occurrences", settings(2_000_000, 1)).await;
    program(&router, case, &multiplex_id, "alpha", 1).await;
    let channel_id = feeding_channel(&router, case, &multiplex_id, "alpha").await;
    wait_for_the_clock();
    run_multiplex(&router, case, &multiplex_id).await;

    // The channel exists but is stopped, so the condition holds: one SET alert.
    let before = std::time::SystemTime::now();
    let (raised, _) = every_alert(&router, case, &multiplex_id, None, None).await;
    let after = std::time::SystemTime::now();
    assert_eq!(
        raised.len(),
        1,
        "case {case:?}: expected the one condition that holds to have raised one alert, got \
         {raised:?}"
    );
    let first = raised[0].clone();
    assert_set_alert(case, &first, "alpha");
    let message = first["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&channel_id) && message.contains("IDLE"),
        "case {case:?}: expected the message to name the channel that is not running and its \
         state, got {message:?}"
    );
    let set_at = epoch_seconds(timestamp(case, "setTimestamp", &first));
    assert!(
        set_at <= crate::support::unix_seconds(after) + 1.0
            && set_at >= crate::support::unix_seconds(before) - 2.0,
        "case {case:?}: expected the set instant to be around the call that observed the \
         condition, got {set_at}"
    );

    // Starting the channel stops the condition holding, so the alert clears —
    // same id, same set instant, now with a cleared instant.
    start_channel(&router, case, &channel_id).await;
    wait_for_the_clock();
    let (cleared, _) = every_alert(&router, case, &multiplex_id, None, None).await;
    assert_eq!(
        cleared.len(),
        1,
        "case {case:?}: expected the cleared alert to be kept, not dropped, got {cleared:?}"
    );
    let clearing = cleared[0].clone();
    assert_eq!(
        alert_id(case, &clearing),
        alert_id(case, &first),
        "case {case:?}: expected the same alert instance, got {clearing}"
    );
    assert_eq!(
        alert_state(case, &clearing),
        multiplex_alert_state::CLEARED,
        "case {case:?}: expected the alert to have cleared, got {clearing}"
    );
    assert_eq!(
        clearing["setTimestamp"], first["setTimestamp"],
        "case {case:?}: expected clearing to leave the set instant alone, got {clearing}"
    );
    let cleared_at = epoch_seconds(timestamp(case, "clearedTimestamp", &clearing));
    assert!(
        cleared_at >= set_at,
        "case {case:?}: expected the cleared instant to be at or after the set one, got \
         {cleared_at} after {set_at}"
    );
    assert_eq!(
        clearing["message"], first["message"],
        "case {case:?}: expected the alert to keep the message it was raised with, got {clearing}"
    );

    // Stopping the channel makes the condition hold again: a NEW instance, with
    // its own id, beside the cleared one.
    stop_channel(&router, case, &channel_id).await;
    wait_for_the_clock();
    let (both, _) = every_alert(&router, case, &multiplex_id, None, None).await;
    assert_eq!(
        both.len(),
        2,
        "case {case:?}: expected a second occurrence beside the cleared first, got {both:?}"
    );
    assert_eq!(
        alert_id(case, &both[0]),
        alert_id(case, &first),
        "case {case:?}: expected the cleared alert to still be reported first, got {both:?}"
    );
    assert_eq!(
        alert_state(case, &both[0]),
        multiplex_alert_state::CLEARED,
        "case {case:?}: expected the first alert to still be cleared, got {both:?}"
    );
    assert_set_alert(case, &both[1], "alpha");
    assert_ne!(
        alert_id(case, &both[1]),
        alert_id(case, &first),
        "case {case:?}: expected the new occurrence to have an id of its own, got {both:?}"
    );
}

/// A multiplex holding `programs` unfed programs, run, stopped and run again, so
/// it carries one CLEARED alert per program (from the first run) followed by one
/// SET alert per program (from the second) — a fixture with both states in it, in
/// the order they were raised.
async fn mixed_alerts(router: &MultiServiceRouter, case: &str, programs: usize) -> String {
    let multiplex_id = multiplex_with_settings(router, case, "mixed", settings(2_000_000, 1)).await;
    for index in 0..programs {
        // Zero-padded, so program name order is index order.
        let name = format!("program-{index:03}");
        program(router, case, &multiplex_id, &name, index as i64 + 1).await;
    }
    wait_for_the_clock();
    run_multiplex(router, case, &multiplex_id).await;
    // The first run's alerts have to be raised before the stop can clear them.
    let (raised, _) = every_alert(router, case, &multiplex_id, None, Some(1)).await;
    assert_eq!(
        raised.len(),
        programs,
        "case {case:?}: expected the first run to raise one alert per program, got {raised:?}"
    );
    let (status, stopped) = stop_multiplex(router, &multiplex_id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StopMultiplex: expected 202, got {status}: {stopped}"
    );
    wait_for_the_clock();
    run_multiplex(router, case, &multiplex_id).await;
    multiplex_id
}

/// Rule: the listing reports the alerts the `StateFilter` admits — only the SET
/// ones, only the CLEARED ones, or every alert for `ALL` and for a request
/// carrying no filter — in the order they were raised, cut into pages of at most
/// the requested page size bounded by the service's own ceiling, whose token
/// resumes at the first alert not yet reported and is absent on the last page.
///
/// The rows differ along the filter asked for (every value the member's
/// documentation enumerates, plus no filter at all) and along the page size
/// asked for (one, a size that divides the collection unevenly, none, and a size
/// past the service's ceiling), over one fixture carrying both alert states —
/// because a listing is a report, so every row reads the same stored alerts.
#[tokio::test]
async fn alerts_are_filtered_by_state_and_paged_in_the_order_they_were_raised() {
    const PROGRAMS: usize = 3;
    let router = medialive();
    let multiplex_id = mixed_alerts(&router, "a multiplex run twice", PROGRAMS).await;

    // The whole collection, as the unfiltered listing reports it: the CLEARED
    // alerts of the first run, then the SET alerts of the second.
    let (all, _) = every_alert(&router, "every alert", &multiplex_id, None, None).await;
    assert_eq!(
        all.len(),
        2 * PROGRAMS,
        "expected one cleared and one set alert per program, got {all:?}"
    );
    let states: Vec<String> = all
        .iter()
        .map(|alert| alert_state("every alert", alert))
        .collect();
    let expected_states: Vec<String> = std::iter::repeat_n(
        multiplex_alert_state::CLEARED.to_string(),
        PROGRAMS,
    )
    .chain(std::iter::repeat_n(
        multiplex_alert_state::SET.to_string(),
        PROGRAMS,
    ))
    .collect();
    assert_eq!(
        states, expected_states,
        "expected the cleared alerts of the first run before the set alerts of the second, \
         got {states:?}"
    );
    let ids: Vec<String> = all
        .iter()
        .map(|alert| alert_id("every alert", alert))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_by_key(|id| id.parse::<u64>().unwrap_or_default());
    assert_eq!(
        ids, sorted,
        "expected the alerts in the order they were raised, got {ids:?}"
    );

    // (case, the StateFilter asked for, the page size asked for)
    let rows: [(&str, Option<&str>, Option<i32>); 10] = [
        ("no filter, no page size", None, None),
        ("no filter, page of one", None, Some(1)),
        ("no filter, page of two", None, Some(2)),
        (
            "no filter, page size past the ceiling",
            None,
            Some(MAX_LIST_PAGE_SIZE as i32 + 500),
        ),
        ("all, no page size", Some(MULTIPLEX_ALERT_STATE_FILTER_ALL), None),
        ("all, page of two", Some(MULTIPLEX_ALERT_STATE_FILTER_ALL), Some(2)),
        ("set only", Some(multiplex_alert_state::SET), None),
        ("set only, page of one", Some(multiplex_alert_state::SET), Some(1)),
        ("cleared only", Some(multiplex_alert_state::CLEARED), None),
        (
            "cleared only, page of two",
            Some(multiplex_alert_state::CLEARED),
            Some(2),
        ),
    ];

    for (case, state_filter, page_size) in rows {
        let expected: Vec<Value> = all
            .iter()
            .filter(|alert| match state_filter {
                None | Some(MULTIPLEX_ALERT_STATE_FILTER_ALL) => true,
                Some(state) => alert_state(case, alert) == state,
            })
            .cloned()
            .collect();

        let (reported, pages) =
            every_alert(&router, case, &multiplex_id, state_filter, page_size).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the alerts the filter admits, each once, in the order they \
             were raised; pages were {pages:?}"
        );

        // The page cut itself: with an effective page size of `n`, every page
        // but the last carries exactly `n`.
        let size = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        let expected_pages = if expected.is_empty() {
            1
        } else {
            expected.len().div_ceil(size)
        };
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} alerts of {size}, got \
             {pages:?}",
            expected.len()
        );
    }
}

/// Rule: the service's own page-size ceiling cuts a listing a request asked no
/// page size for, so a multiplex carrying more alerts than one page holds still
/// reports every one of them across the pages its token walks.
#[tokio::test]
async fn the_default_page_size_cuts_a_long_alert_listing() {
    let case = "more alerts than one default page holds";
    // Two occurrences per program (one cleared, one set), so this many programs
    // carry more alerts than the ceiling admits in one page.
    let programs = MAX_LIST_PAGE_SIZE / 2 + 2;
    let router = medialive();
    let multiplex_id = mixed_alerts(&router, case, programs).await;

    let (first, token) = page(&router, case, &multiplex_id, "").await;
    assert_eq!(
        first.len(),
        MAX_LIST_PAGE_SIZE,
        "case {case:?}: expected the default page to carry the service's ceiling, got {}",
        first.len()
    );
    assert!(
        token.is_some(),
        "case {case:?}: expected a token beside a full page with alerts left over"
    );

    let (every, pages) = every_alert(&router, case, &multiplex_id, None, None).await;
    assert_eq!(
        every.len(),
        2 * programs,
        "case {case:?}: expected every alert across the pages, got {} in pages {pages:?}",
        every.len()
    );
    assert_eq!(
        pages,
        vec![MAX_LIST_PAGE_SIZE, 2 * programs - MAX_LIST_PAGE_SIZE],
        "case {case:?}: expected the ceiling to cut the listing, got {pages:?}"
    );
}

/// Rule: a request the listing cannot serve as asked is refused rather than
/// answered — a `MultiplexId` no multiplex of the account has is the modelled
/// `NotFoundException` (404) naming it, and a page size that asks for nothing, a
/// `StateFilter` outside the three values its documentation enumerates and a
/// `NextToken` this operation never handed out are each the modelled
/// `BadRequestException` (400); every value the member's documentation does
/// enumerate succeeds.
#[tokio::test]
async fn a_request_the_listing_cannot_serve_is_refused() {
    let case = "a multiplex holding one alerting program";
    let router = medialive();
    let multiplex_id = mixed_alerts(&router, case, 1).await;

    // A multiplex that exists answers an empty page for a filter nothing
    // matches, never a refusal — and a multiplex id no multiplex has is a 404.
    for unknown in ["6999999", "0", "not-an-id"] {
        let (status, body) = list(&router, unknown, "").await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected an unknown multiplex id to be refused 404, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(unknown),
            "case {unknown:?}: expected the refusal to name the identifier the request carried, \
             got {:?}",
            error_message(&body)
        );
    }

    // A page size that asks for no results at all is not a page size.
    for max_results in [0, -1, -50] {
        let (status, body) = list(&router, &multiplex_id, &format!("?maxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case {max_results}: expected a non-positive maxResults to be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {max_results}: expected BadRequestException, got {body}"
        );
    }
    // The published bound: one result per page is a page size, and succeeds.
    let (smallest, _) = page(&router, "maxResults=1", &multiplex_id, "?maxResults=1").await;
    assert_eq!(
        smallest.len(),
        1,
        "case \"maxResults=1\": expected a page of one, got {smallest:?}"
    );

    // Every `StateFilter` the member's documentation enumerates succeeds — the
    // catalog comes from the state slice that quotes it.
    for state_filter in MULTIPLEX_ALERT_STATE_FILTERS {
        let (status, body) = list(&router, &multiplex_id, &format!("?stateFilter={state_filter}")).await;
        assert_eq!(
            status, 200,
            "case {state_filter:?}: expected a documented stateFilter to be accepted, got \
             {status}: {body}"
        );
    }
    // And a value outside that catalog — including the right word in the wrong
    // case — is a bad request naming the member and the values it accepts.
    for state_filter in ["set", "Cleared", "ANY", "SET,CLEARED", ""] {
        let (status, body) = list(&router, &multiplex_id, &format!("?stateFilter={state_filter}")).await;
        assert_eq!(
            status, 400,
            "case {state_filter:?}: expected an undocumented stateFilter to be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {state_filter:?}: expected BadRequestException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("stateFilter")
                && MULTIPLEX_ALERT_STATE_FILTERS
                    .iter()
                    .all(|value| message.contains(value)),
            "case {state_filter:?}: expected the refusal to name the member and the values it \
             accepts, got {message:?}"
        );
    }

    // A `NextToken` that is not the id form this operation hands a token out as
    // is a bad request; the one it did hand out resumes the walk. A numeric
    // token no alert has is a RESUME POINT past that id, not a refusal, which is
    // what lets a page resume after an alert the bounded store has evicted.
    for next_token in ["abc", "7000001x", "-1", "1.5", ""] {
        let (status, body) = list(&router, &multiplex_id, &format!("?nextToken={next_token}")).await;
        assert_eq!(
            status, 400,
            "case {next_token:?}: expected a value that is not an alert id to be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {next_token:?}: expected BadRequestException, got {body}"
        );
    }
    for next_token in ["0", "9999999999"] {
        let (resumed, token) = page(
            &router,
            "a token past every alert",
            &multiplex_id,
            &format!("?nextToken={next_token}"),
        )
        .await;
        assert!(
            token.is_none(),
            "case {next_token:?}: expected no token beside a resumed page, got {token:?}"
        );
        // "0" is before every minted id, so it resumes at the first alert;
        // a token past every id resumes at none of them.
        let expected = if next_token == "0" { 2 } else { 0 };
        assert_eq!(
            resumed.len(),
            expected,
            "case {next_token:?}: expected the walk to resume at the first alert at or after the \
             token, got {resumed:?}"
        );
    }
    let (first, token) = page(&router, "a handed-out token", &multiplex_id, "?maxResults=1").await;
    let token = token.unwrap_or_else(|| panic!("expected a token beside the first of two alerts"));
    let (second, _) = page(
        &router,
        "a handed-out token",
        &multiplex_id,
        &format!("?nextToken={token}"),
    )
    .await;
    assert_eq!(
        second.len(),
        1,
        "expected the token to resume at the one alert left over, got {second:?}"
    );
    assert_ne!(
        alert_id("a handed-out token", &second[0]),
        alert_id("a handed-out token", &first[0]),
        "expected the token to resume AFTER the alerts already reported, got {second:?}"
    );
}

/// Rule: the listing is a report, so it never changes what a later caller sees —
/// repeating it answers the same alerts, with the same ids and the same instants,
/// and the multiplex it names is left where it was.
///
/// The rows differ along how the listing is paged and filtered, because a
/// token-resumed walk is where a reporting call would most easily consume state.
#[tokio::test]
async fn listing_alerts_changes_nothing_a_later_caller_sees() {
    let case = "a multiplex run twice";
    let router = medialive();
    let multiplex_id = mixed_alerts(&router, case, 2).await;

    let (status, before) = describe_multiplex(&router, &multiplex_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {before}"
    );

    for (row, state_filter, page_size) in [
        ("unpaged, unfiltered", None, None),
        ("page of one", None, Some(1)),
        ("set only, page of one", Some(multiplex_alert_state::SET), Some(1)),
        (
            "cleared only, unpaged",
            Some(multiplex_alert_state::CLEARED),
            None,
        ),
    ] {
        let (first, _) = every_alert(&router, row, &multiplex_id, state_filter, page_size).await;
        let (second, _) = every_alert(&router, row, &multiplex_id, state_filter, page_size).await;
        assert_eq!(
            second, first,
            "case {row:?}: expected a repeated listing to report the same alerts, got {second:?} \
             after {first:?}"
        );
    }

    let (status, after) = describe_multiplex(&router, &multiplex_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex after the listings: expected 200, got {status}: {after}"
    );
    assert_eq!(
        after, before,
        "case {case:?}: expected the listings to leave the multiplex where it was, got {after}"
    );
}
