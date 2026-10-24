// SPDX-License-Identifier: CC0-1.0

//! One channel's ALERTS, end to end over the restJson1 wire: `ListAlerts`.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! An alert is the SERVICE's observation of a condition on a channel, and this
//! emulator ingests no media, so no operation of it raises one: the alerts a
//! suite reads back are recorded in the state the service reads
//! (`ChannelRecord::raise_alert`, the store's own writer, which is what keeps
//! every recorded alert's state and timestamps consistent). The read under test
//! still crosses the HTTP boundary. A channel nothing has recorded an alert on —
//! every channel a client of this emulator creates — is one row of the tables
//! below, not the whole suite.
//!
//! The rules each suite asserts are stated on it, and the rows differ along what
//! the rule generalises over: the alert's members, its state, the channel class
//! that decides which pipelines exist, the `StateFilter` values, the page sizes
//! and the tokens.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use serde_json::json;
use service_com_amazonaws_medialive_medialive::state::{
    channel_alert_state, not_found_message, service_state, CHANNEL_ALERT_FILTERS,
    MAX_CHANNEL_ALERTS, MAX_LIST_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{epoch_seconds, member, percent_encoded, timestamp, unix_seconds};
use crate::wire::{call, error_code, error_message, medialive_with_engine};

/// The engine the router under test was built on, so a suite can record on it
/// the alerts no operation of this emulator produces.
type Engine = Arc<engine::InMemoryEngine>;

/// A created channel of `class` (absent for the service's default), answering
/// its id.
async fn channel(router: &MultiServiceRouter, name: &str, class: Option<&str>) -> String {
    let mut request = json!({ "name": name });
    if let Some(class) = class {
        request["channelClass"] = json!(class);
    }
    let (status, created) = call(router, "POST", "/prod/channels", Some(request)).await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {created}"
    );
    created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// An instant `offset` seconds into a fixed base, so the set order a suite seeds
/// is the order it asserts and nothing depends on the wall clock.
fn at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset)
}

/// Record one alert on `channel_id` through the store's own writer, answering
/// the `Id` it minted — and `None` when the writer refuses the pipeline the
/// observation named, which is the invariant
/// `an_alert_is_recorded_only_for_a_pipeline_the_channel_class_gives_it` reads.
fn try_raise(
    engine: &Engine,
    channel_id: &str,
    alert_type: &str,
    message: Option<&str>,
    pipeline: Option<i32>,
    set_at: SystemTime,
) -> Option<String> {
    let store = service_state(&**engine).expect("the medialive store");
    let mut store = store.lock();
    let channel = store
        .channels
        .get_mut(channel_id)
        .unwrap_or_else(|| panic!("setup: channel {channel_id} must be in the store"));
    channel.raise_alert(alert_type, message, pipeline, set_at)
}

/// [`try_raise`] for a seeding call that must have been recorded.
fn raise(
    engine: &Engine,
    channel_id: &str,
    alert_type: &str,
    message: Option<&str>,
    pipeline: Option<i32>,
    set_at: SystemTime,
) -> String {
    try_raise(engine, channel_id, alert_type, message, pipeline, set_at).unwrap_or_else(|| {
        panic!("setup: the alert on channel {channel_id} must have been recorded")
    })
}

/// Clear a recorded alert — "the alert is no longer happening" — as of
/// `cleared_at`.
fn clear(engine: &Engine, channel_id: &str, alert_id: &str, cleared_at: SystemTime) {
    let store = service_state(&**engine).expect("the medialive store");
    let mut store = store.lock();
    let channel = store
        .channels
        .get_mut(channel_id)
        .unwrap_or_else(|| panic!("setup: channel {channel_id} must be in the store"));
    assert!(
        channel.alerts.clear(alert_id, cleared_at),
        "setup: alert {alert_id} must have been SET before it is cleared"
    );
}

/// How many alerts the channel's log holds, whatever state they are in.
fn logged(engine: &Engine, channel_id: &str) -> usize {
    let store = service_state(&**engine).expect("the medialive store");
    let store = store.lock();
    store
        .channels
        .get(channel_id)
        .map_or(0, |channel| channel.alerts.len())
}

/// `ListAlerts` as a client calls it: GET /prod/channels/{ChannelId}/alerts with
/// the request's `@httpQuery` members
/// (`smithy_query(query="operation", operation="ListAlerts")`).
async fn list(
    router: &MultiServiceRouter,
    channel_id: &str,
    query: &[(&str, String)],
) -> (u16, Value) {
    let mut uri = format!("/prod/channels/{channel_id}/alerts");
    if !query.is_empty() {
        let encoded: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={}", percent_encoded(value)))
            .collect();
        uri.push('?');
        uri.push_str(&encoded.join("&"));
    }
    call(router, "GET", &uri, None).await
}

/// The `alerts` the response reported, as JSON objects in the order they were
/// reported.
fn reported(body: &Value) -> Vec<Value> {
    body["alerts"].as_array().cloned().unwrap_or_default()
}

/// The `id` of each reported alert, in the order they were reported.
fn reported_ids(body: &Value) -> Vec<String> {
    reported(body)
        .iter()
        .map(|alert| alert["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Page through every alert of one channel the `filter` reports, answering their
/// ids in the order they were reported.
///
/// Asserts the pagination contract on the way: no page is larger than the page
/// size in force, a page that hands out a token is not empty, and the walk ends
/// with a response that carries no token.
async fn walk(
    router: &MultiServiceRouter,
    channel_id: &str,
    filter: Option<&str>,
    max_results: Option<i32>,
    case: &str,
) -> Vec<String> {
    let cap = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| {
        (size as usize).min(MAX_LIST_PAGE_SIZE)
    });
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(filter) = filter {
            query.push(("stateFilter", filter.to_string()));
        }
        if let Some(size) = max_results {
            query.push(("maxResults", size.to_string()));
        }
        if let Some(token) = &token {
            query.push(("nextToken", token.clone()));
        }
        let (status, body) = list(router, channel_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAlerts page {pages} to answer 200, got {status}: {body}"
        );
        let page = reported_ids(&body);
        assert!(
            page.len() <= cap,
            "case {case:?}: expected at most {cap} alerts in page {pages}, got {}: {body}",
            page.len()
        );
        ids.extend(page.clone());
        pages += 1;
        assert!(
            pages <= 4 * MAX_CHANNEL_ALERTS,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that hands out the token {next:?} must have \
                     reported at least one alert: {body}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    ids
}

/// One seeded alert: the row of the projection table below.
struct Row {
    case: &'static str,
    /// `ChannelAlert$AlertType` is "The type of the alert", a plain `__string`
    /// the model constrains with no value list
    /// (`smithy_query(query="shape", shape="ChannelAlert")`), so a row carries
    /// whatever type the observation had; the rule under test is the projection,
    /// not a catalog of types.
    alert_type: &'static str,
    message: Option<&'static str>,
    pipeline: Option<i32>,
    cleared: bool,
}

/// Rule: `ListAlerts` reports every alert recorded on the channel the path names
/// — each member exactly as the store holds it, with `ClearedTimestamp` carried
/// if and only if the alert has been cleared — in the order the alerts were set.
///
/// The table spans what the rule generalises over: whether the alert carries a
/// message at all (and an empty one, which is a message), whether it names a
/// pipeline, and the two `ChannelAlertState` values
/// (`smithy_query(query="shape", shape="ChannelAlertState")`: SET | CLEARED).
#[tokio::test]
async fn every_recorded_alert_is_reported_as_the_store_holds_it() {
    let table: [Row; 6] = [
        Row {
            case: "set, pipeline 0, with a message",
            alert_type: "InputNotDetected",
            message: Some("the input is not delivering media"),
            pipeline: Some(0),
            cleared: false,
        },
        Row {
            case: "set, pipeline 1, no message",
            alert_type: "OutputDisabled",
            message: None,
            pipeline: Some(1),
            cleared: false,
        },
        Row {
            case: "set, no pipeline",
            alert_type: "RoleArnNotAssumable",
            message: Some("the channel's role cannot be assumed"),
            pipeline: None,
            cleared: false,
        },
        Row {
            case: "cleared, pipeline 1",
            alert_type: "InputNotDetected",
            message: Some("the input is not delivering media"),
            pipeline: Some(1),
            cleared: true,
        },
        Row {
            case: "cleared, no pipeline, empty message",
            alert_type: "ChannelDegraded",
            message: Some(""),
            pipeline: None,
            cleared: true,
        },
        Row {
            case: "set, a message with non-ASCII text",
            alert_type: "InputVideoFrozen",
            message: Some("vidéo gelée — 입력 정지"),
            pipeline: Some(0),
            cleared: false,
        },
    ];

    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "alerted-channel", Some("STANDARD")).await;

    // Seeded in row order at one-second intervals, so the set order the rule
    // reports in is the row order.
    let mut seeded: Vec<(usize, String, SystemTime, Option<SystemTime>)> = Vec::new();
    for (index, row) in table.iter().enumerate() {
        let set_at = at(index as u64);
        let id = raise(
            &engine,
            &channel_id,
            row.alert_type,
            row.message,
            row.pipeline,
            set_at,
        );
        let cleared_at = row.cleared.then(|| {
            let cleared_at = at(index as u64 + 500);
            clear(&engine, &channel_id, &id, cleared_at);
            cleared_at
        });
        seeded.push((index, id, set_at, cleared_at));
    }

    let (status, body) = list(&router, &channel_id, &[]).await;
    assert_eq!(status, 200, "ListAlerts: expected 200, got {status}: {body}");
    let alerts = reported(&body);
    assert_eq!(
        alerts.len(),
        table.len(),
        "expected every recorded alert to be reported, got {body}"
    );
    assert_eq!(
        body.get("nextToken"),
        None,
        "a single page holding every alert must hand out no token, got {body}"
    );

    for (index, id, set_at, cleared_at) in seeded {
        let row = &table[index];
        let case = row.case;
        let alert = &alerts[index];
        assert_eq!(
            alert["id"],
            json!(id),
            "case {case:?}: expected the alerts reported in set order, so position {index} \
             must be alert {id}, got {alert}"
        );
        assert_eq!(
            alert["alertType"],
            json!(row.alert_type),
            "case {case:?}: expected alertType {:?}, got {alert}",
            row.alert_type
        );
        assert_eq!(
            member(alert, "message"),
            row.message.map_or(Value::Null, |message| json!(message)),
            "case {case:?}: expected message {:?}, got {alert}",
            row.message
        );
        assert_eq!(
            member(alert, "pipelineId"),
            row.pipeline
                .map_or(Value::Null, |pipeline| json!(pipeline.to_string())),
            "case {case:?}: expected pipelineId {:?}, got {alert}",
            row.pipeline
        );
        let expected_state = if row.cleared {
            channel_alert_state::CLEARED
        } else {
            channel_alert_state::SET
        };
        assert_eq!(
            alert["state"],
            json!(expected_state),
            "case {case:?}: expected state {expected_state}, got {alert}"
        );
        let reported_set = epoch_seconds(timestamp(case, "setTimestamp", alert));
        assert!(
            (reported_set - unix_seconds(set_at)).abs() < 0.001,
            "case {case:?}: expected setTimestamp {} seconds, got {reported_set}",
            unix_seconds(set_at)
        );
        match cleared_at {
            Some(cleared_at) => {
                let reported_cleared =
                    epoch_seconds(timestamp(case, "clearedTimestamp", alert));
                assert!(
                    (reported_cleared - unix_seconds(cleared_at)).abs() < 0.001,
                    "case {case:?}: expected clearedTimestamp {} seconds, got {reported_cleared}",
                    unix_seconds(cleared_at)
                );
            }
            None => assert_eq!(
                member(alert, "clearedTimestamp"),
                Value::Null,
                "case {case:?}: an alert that is still SET has no clearedTimestamp, got {alert}"
            ),
        }
    }
}

/// Rule: an alert names a pipeline only when the channel's class actually gives
/// it that pipeline — `ChannelAlert$PipelineId` is "The ID of the pipeline this
/// alert is associated with", numbered as `PipelineDetail$PipelineId` numbers
/// them, and a `SINGLE_PIPELINE` channel "only has one" while a `STANDARD` one
/// "has two encoding pipelines" (`smithy_query(query="shape",
/// shape="ChannelClass")`).
///
/// The table spans the `ChannelClass` enum against the pipeline numbers inside
/// and outside each class's range, including the negative one. A pipeline the
/// class has not got is not one an alert can be associated with, so no alert is
/// recorded for it at all and the channel reports none.
#[tokio::test]
async fn an_alert_reports_only_a_pipeline_the_channel_class_gives_it() {
    // (case, ChannelClass, pipeline the observation named, reported pipelineId)
    let table: [(&str, &str, Option<i32>, Option<&str>); 7] = [
        ("standard, pipeline 0", "STANDARD", Some(0), Some("0")),
        ("standard, pipeline 1", "STANDARD", Some(1), Some("1")),
        ("standard, pipeline 2 is past the class", "STANDARD", Some(2), None),
        ("standard, negative pipeline", "STANDARD", Some(-1), None),
        ("single pipeline, pipeline 0", "SINGLE_PIPELINE", Some(0), Some("0")),
        (
            "single pipeline, pipeline 1 is past the class",
            "SINGLE_PIPELINE",
            Some(1),
            None,
        ),
        ("standard, no pipeline named", "STANDARD", None, Some("<none>")),
    ];

    for (case, class, named, expected) in table {
        let (router, engine) = medialive_with_engine();
        let channel_id = channel(&router, "pipeline-alerted", Some(class)).await;
        let recorded = try_raise(&engine, &channel_id, "InputNotDetected", None, named, at(0));
        // An alert about the channel itself (no pipeline) and one about a
        // pipeline the class gives it are recorded; one about a pipeline the
        // class has not got is refused, which is what `expected` being absent
        // means.
        assert_eq!(
            recorded.is_some(),
            expected.is_some(),
            "case {case:?}: expected the writer to record the alert: {}, got {recorded:?}",
            expected.is_some()
        );

        let (status, body) = list(&router, &channel_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAlerts to answer 200, got {status}: {body}"
        );
        let alerts = reported(&body);
        let Some(expected) = expected else {
            assert_eq!(
                body["alerts"],
                json!([]),
                "case {case:?}: an alert naming a pipeline the class has not got is \
                 recorded nowhere, so the channel reports none, got {body}"
            );
            continue;
        };
        assert_eq!(
            alerts.len(),
            1,
            "case {case:?}: expected the one recorded alert, got {body}"
        );
        assert_eq!(
            member(&alerts[0], "pipelineId"),
            if expected == "<none>" {
                Value::Null
            } else {
                json!(expected)
            },
            "case {case:?}: expected pipelineId {expected:?}, got {}",
            alerts[0]
        );
    }
}

/// Rule: `StateFilter` "Specifies the set of alerts to return based on their
/// state. SET - Return only alerts with SET state. CLEARED - Return only alerts
/// with CLEARED state. ALL - Return all alerts." — and a request naming no
/// filter narrows nothing, so it asks for them all.
///
/// The table spans the three documented values plus the absent one, against a
/// log holding alerts in both states.
#[tokio::test]
async fn the_state_filter_reports_exactly_the_alerts_in_that_state() {
    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "filtered-channel", None).await;

    let mut set_ids: Vec<String> = Vec::new();
    let mut cleared_ids: Vec<String> = Vec::new();
    for index in 0..5u64 {
        let id = raise(
            &engine,
            &channel_id,
            "InputNotDetected",
            Some("seeded"),
            Some((index % 2) as i32),
            at(index),
        );
        // Every second alert is no longer happening, so both states are
        // represented and neither is the whole log.
        if index % 2 == 0 {
            clear(&engine, &channel_id, &id, at(index + 500));
            cleared_ids.push(id);
        } else {
            set_ids.push(id);
        }
    }
    let all_ids: Vec<String> = {
        let mut ids = set_ids.clone();
        ids.extend(cleared_ids.clone());
        // Reported in set order, which is the seeding order, not the order the
        // two buckets were collected in.
        ids.sort();
        ids
    };

    // (case, StateFilter the request carried, the alerts it must report)
    let table: [(&str, Option<&str>, &Vec<String>); 4] = [
        ("no filter at all", None, &all_ids),
        ("ALL", Some("ALL"), &all_ids),
        ("SET", Some("SET"), &set_ids),
        ("CLEARED", Some("CLEARED"), &cleared_ids),
    ];

    for (case, filter, expected) in table {
        let query: Vec<(&str, String)> = filter
            .map(|filter| vec![("stateFilter", filter.to_string())])
            .unwrap_or_default();
        let (status, body) = list(&router, &channel_id, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAlerts to answer 200, got {status}: {body}"
        );
        let mut ids = reported_ids(&body);
        let states: Vec<Value> = reported(&body)
            .iter()
            .map(|alert| alert["state"].clone())
            .collect();
        if let Some(filter) = filter.filter(|filter| *filter != "ALL") {
            assert!(
                states.iter().all(|state| *state == json!(filter)),
                "case {case:?}: expected every reported alert to be {filter}, got {states:?}"
            );
        }
        ids.sort();
        let mut expected = expected.clone();
        expected.sort();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected the alerts {expected:?}, got {body}"
        );
    }
}

/// Rule: a `StateFilter` outside the set its own documentation names asks for a
/// set of alerts the service cannot name, so the request is refused with the
/// operation's modelled 400 `BadRequestException` rather than answered with some
/// other set.
///
/// The table spans the ways a value falls outside that set: a value of the right
/// shape that is not one of them, each documented value in the wrong case (an
/// enum value in this API is case-sensitive), one with trailing whitespace, and
/// a `ChannelState` value, which belongs to the channel and not to its alerts.
#[tokio::test]
async fn a_state_filter_outside_the_documented_values_is_refused() {
    let table: [&str; 7] = ["set", "cleared", "all", "SET ", "RUNNING", "NONE", "Set"];

    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "bad-filter-channel", None).await;
    let id = raise(&engine, &channel_id, "InputNotDetected", None, Some(0), at(0));

    for case in table {
        let (status, body) = list(
            &router,
            &channel_id,
            &[("stateFilter", case.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a stateFilter outside {CHANNEL_ALERT_FILTERS:?} to be \
             refused with 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("stateFilter") && message.contains(case),
            "case {case:?}: expected the refusal to name the member and the value it \
             carried, got {message:?}"
        );
    }

    // The refused requests changed nothing: the log still holds the one alert.
    let (status, body) = list(&router, &channel_id, &[]).await;
    assert_eq!(status, 200, "expected ListAlerts to answer 200, got {status}: {body}");
    assert_eq!(
        reported_ids(&body),
        vec![id],
        "a refused filter must leave the log alone, got {body}"
    );
}

/// Rule: `MaxResults` is "The maximum number of items to return", so a response
/// carries at most that many alerts and hands out a `NextToken` exactly while
/// alerts are left over; walking the tokens reports every alert the filter
/// selects, once each, in set order.
///
/// The table spans the page sizes the rule generalises over: one at a time, a
/// size that divides the log, one that does not, exactly the log's size, one
/// past it, a size above the service's own page ceiling (clamped, not refused)
/// and the absent size.
#[tokio::test]
async fn pagination_reports_every_selected_alert_once_in_set_order() {
    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "paged-channel", Some("STANDARD")).await;

    let mut expected_all: Vec<String> = Vec::new();
    let mut expected_set: Vec<String> = Vec::new();
    let mut expected_cleared: Vec<String> = Vec::new();
    for index in 0..7u64 {
        let id = raise(
            &engine,
            &channel_id,
            "InputNotDetected",
            Some("paged"),
            Some((index % 2) as i32),
            at(index),
        );
        if index % 3 == 0 {
            clear(&engine, &channel_id, &id, at(index + 500));
            expected_cleared.push(id.clone());
        } else {
            expected_set.push(id.clone());
        }
        expected_all.push(id);
    }

    let sizes: [Option<i32>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(7),
        Some(8),
        Some(MAX_LIST_PAGE_SIZE as i32 + 1),
        None,
    ];
    // (filter, the alerts it selects, in set order)
    let selections: [(Option<&str>, &Vec<String>); 4] = [
        (None, &expected_all),
        (Some("ALL"), &expected_all),
        (Some("SET"), &expected_set),
        (Some("CLEARED"), &expected_cleared),
    ];

    for (filter, expected) in selections {
        for size in sizes {
            let case = format!("filter {filter:?}, maxResults {size:?}");
            let walked = walk(&router, &channel_id, filter, size, &case).await;
            assert_eq!(
                &walked, expected,
                "case {case:?}: expected the walk to report {expected:?} in set order, got \
                 {walked:?}"
            );
        }
    }
}

/// Rule: a request this operation cannot read is refused with its modelled 400 —
/// a page size that asks for nothing, and a `NextToken` that is not a position
/// in the sequence — while a token for a position PAST the end is a valid walk
/// of a log that has since shrunk, so it answers an empty last page.
#[tokio::test]
async fn an_unreadable_page_size_or_token_is_refused() {
    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "refusing-channel", None).await;
    for index in 0..3u64 {
        raise(&engine, &channel_id, "InputNotDetected", None, None, at(index));
    }

    // A page size at or below zero asks the service for no items at all.
    for size in [0, -1, -1000] {
        let (status, body) = list(&router, &channel_id, &[("maxResults", size.to_string())]).await;
        assert_eq!(
            status, 400,
            "case {size}: expected a maxResults below 1 to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {size}: expected BadRequestException, got {body}"
        );
    }

    // A token is a position in the sequence; anything else is not a continuation.
    for token in ["abc", "-1", "1.5", "two", "0x1"] {
        let (status, body) = list(&router, &channel_id, &[("nextToken", token.to_string())]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected a token that is not a position to be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {token:?}: expected BadRequestException, got {body}"
        );
    }

    // A position past the end is a walk of a log that has shrunk since the token
    // was handed out: the last page is empty and carries no token.
    for token in ["3", "4", "1000"] {
        let (status, body) = list(&router, &channel_id, &[("nextToken", token.to_string())]).await;
        assert_eq!(
            status, 200,
            "case {token:?}: expected a position past the end to answer 200, got {status}: {body}"
        );
        assert_eq!(
            reported_ids(&body),
            Vec::<String>::new(),
            "case {token:?}: expected an empty last page, got {body}"
        );
        assert_eq!(
            body.get("nextToken"),
            None,
            "case {token:?}: an empty last page hands out no token, got {body}"
        );
    }
}

/// Rule: a channel on which the service has observed no alert reports an empty
/// alert list — a 200 with no alerts and no token — whatever lifecycle state the
/// channel is in. Nothing in this emulator ingests media, so this is what a
/// client of it sees for a channel it created, and it is never an alert the
/// service invented out of the channel's state.
///
/// The table spans the channel states a created channel reaches
/// (`smithy_query(query="shape", shape="ChannelState")`) and both channel
/// classes, so a running channel with every pipeline up reports no alerts just
/// as a creating one does.
#[tokio::test]
async fn a_channel_with_no_observed_alert_reports_an_empty_list() {
    // (case, ChannelClass, whether to let the create settle, whether to start it)
    let table: [(&str, Option<&str>, bool, bool); 5] = [
        ("CREATING, standard", Some("STANDARD"), false, false),
        ("IDLE, standard", Some("STANDARD"), true, false),
        ("RUNNING, standard", Some("STANDARD"), true, true),
        ("IDLE, single pipeline", Some("SINGLE_PIPELINE"), true, false),
        ("RUNNING, class defaulted", None, true, true),
    ];

    for (case, class, settle, start) in table {
        let (router, engine) = medialive_with_engine();
        let channel_id = channel(&router, "quiet-channel", class).await;
        if settle {
            wait_for_the_clock();
        }
        if start {
            let (status, body) = call(
                &router,
                "POST",
                &format!("/prod/channels/{channel_id}/start"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartChannel: expected 200, got {status}: {body}"
            );
            wait_for_the_clock();
        }

        for filter in [None, Some("ALL"), Some("SET"), Some("CLEARED")] {
            let query: Vec<(&str, String)> = filter
                .map(|filter| vec![("stateFilter", filter.to_string())])
                .unwrap_or_default();
            let (status, body) = list(&router, &channel_id, &query).await;
            assert_eq!(
                status, 200,
                "case {case:?} filter {filter:?}: expected ListAlerts to answer 200, never the \
                 scaffold's 501, got {status}: {body}"
            );
            assert_eq!(
                body["alerts"],
                json!([]),
                "case {case:?} filter {filter:?}: expected the empty alert list the store \
                 holds, got {body}"
            );
            assert_eq!(
                body.get("nextToken"),
                None,
                "case {case:?} filter {filter:?}: an empty page hands out no token, got {body}"
            );
        }
        let logged_now = logged(&engine, &channel_id);
        assert_eq!(
            logged_now, 0,
            "case {case:?}: listing alerts must not record one, got {logged_now} in the log"
        );
    }
}

/// Rule: `ChannelId` is "The unique ID of the channel", so an id that names no
/// channel of this account is the operation's modelled 404 `NotFoundException`,
/// worded the way every other not-found refusal of this service is.
#[tokio::test]
async fn an_id_that_names_no_channel_is_a_not_found() {
    let (router, engine) = medialive_with_engine();
    // One real channel with a real alert, so a 404 is about the id in the path
    // and not about an empty store.
    let channel_id = channel(&router, "present-channel", None).await;
    raise(&engine, &channel_id, "InputNotDetected", None, Some(0), at(0));

    for absent in ["0", "9999999", "not-a-channel", "1000000000000"] {
        assert_ne!(
            absent, channel_id,
            "setup: the absent id {absent:?} must not be the created channel"
        );
        for query in [
            Vec::new(),
            vec![("stateFilter", "SET".to_string())],
            vec![("maxResults", "1".to_string())],
        ] {
            let (status, body) = list(&router, absent, &query).await;
            assert_eq!(
                status, 404,
                "case {absent:?} with {query:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {absent:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                not_found_message("Channel", absent),
                "case {absent:?}: expected the service's shared not-found wording, got {body}"
            );
        }
    }
}

/// Rule: listing alerts is a REPORT — it changes nothing a later caller sees, so
/// the same read answers the same alerts however many times, and whatever pages
/// and filters were asked for in between.
#[tokio::test]
async fn listing_alerts_changes_nothing_a_later_caller_sees() {
    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "reported-channel", Some("STANDARD")).await;
    for index in 0..4u64 {
        let id = raise(
            &engine,
            &channel_id,
            "InputNotDetected",
            Some("reported"),
            Some((index % 2) as i32),
            at(index),
        );
        if index == 1 {
            clear(&engine, &channel_id, &id, at(index + 500));
        }
    }

    let (status, before) = list(&router, &channel_id, &[]).await;
    assert_eq!(status, 200, "expected ListAlerts to answer 200, got {status}: {before}");
    let logged_before = logged(&engine, &channel_id);

    // Every shape of read in between: filtered, paged, refused and not found.
    for query in [
        vec![("stateFilter", "SET".to_string())],
        vec![("stateFilter", "CLEARED".to_string())],
        vec![("maxResults", "1".to_string())],
        vec![("maxResults", "2".to_string()), ("nextToken", "2".to_string())],
        vec![("stateFilter", "set".to_string())],
    ] {
        let (_status, _body) = list(&router, &channel_id, &query).await;
    }
    let (status, after) = list(&router, &channel_id, &[]).await;
    assert_eq!(status, 200, "expected ListAlerts to answer 200, got {status}: {after}");
    assert_eq!(
        after, before,
        "a read must report the same alerts it reported before: expected {before}, got {after}"
    );
    assert_eq!(
        logged(&engine, &channel_id),
        logged_before,
        "a read must neither raise nor clear an alert"
    );
}

/// Rule: the alert log a channel keeps is BOUNDED
/// (`state::MAX_CHANNEL_ALERTS`) — the API deletes no alert and no doc sentence
/// pins a retention, so once it is full the newest observations are the ones
/// reported and the oldest ones are gone.
#[tokio::test]
async fn a_full_log_reports_the_newest_alerts() {
    let (router, engine) = medialive_with_engine();
    let channel_id = channel(&router, "noisy-channel", None).await;

    // Three past the bound, so the drop is observable and the bound itself is
    // read from the crate rather than written twice.
    let raised: Vec<String> = (0..MAX_CHANNEL_ALERTS + 3)
        .map(|index| {
            raise(
                &engine,
                &channel_id,
                "InputNotDetected",
                None,
                None,
                at(index as u64),
            )
        })
        .collect();

    assert_eq!(
        logged(&engine, &channel_id),
        MAX_CHANNEL_ALERTS,
        "expected the log to hold at most {MAX_CHANNEL_ALERTS} alerts"
    );
    let reported = walk(&router, &channel_id, None, None, "full log").await;
    let expected: Vec<String> = raised[raised.len() - MAX_CHANNEL_ALERTS..].to_vec();
    assert_eq!(
        reported, expected,
        "expected the newest {MAX_CHANNEL_ALERTS} alerts in set order, got {} alerts \
         starting at {:?}",
        reported.len(),
        reported.first()
    );
    for dropped in &raised[..3] {
        assert!(
            !reported.contains(dropped),
            "expected the oldest alert {dropped:?} to have been dropped from the full log"
        );
    }
}
