// SPDX-License-Identifier: CC0-1.0

//! The INSTANT a delivered `RumEvent` occurred at, as the data plane persists it.
//!
//! The rule under test: a `@required` instant a client delivered is persisted in a form
//! the service reads back as the same instant, so every event `PutRumEvents` acknowledges
//! is reported by `GetAppMonitorData` for a time range covering it — and only for such a
//! range — and the copy stored in CloudWatch Logs still says when the event occurred.
//! Rendering an instant as RFC 3339 is fallible (the grammar spells the year as exactly
//! four digits, so an instant far enough from the epoch has no rendering at all), and a
//! failure to render is not a licence to drop the instant.
//!
//! The rows span what the rule generalises over — the value of the instant: an ordinary
//! one, the epoch, the second before it, each end of the range RFC 3339 can spell, one
//! second past each end, instants far outside it in both directions, and one carrying a
//! subsecond part.

use serde_json::{json, Value};

use crate::authorization::{Emulator, ACCOUNT_KEY};

/// One instant a batch delivers, and what the stored (and reported, and copied) document
/// must then carry for it.
struct Row {
    case: &'static str,
    /// `RumEvent$timestamp` as the wire carries it: seconds since the epoch, which is the
    /// form restJson1 carries a `Timestamp` in. An unconstrained `@required` timestamp
    /// (`smithy_query(query="shape", shape="RumEvent")` gives its `timestamp` member no
    /// `@range`), so every value below is one a client may send.
    delivered: Value,
    /// The whole second the instant falls in: the unit `TimeRange`'s `After` and `Before`
    /// bound an event at (both are `QueryTimestamp`, a `long`).
    seconds: i64,
    /// The `timestamp` member the stored document must carry — the RFC 3339 instant where
    /// the instant renders, and the seconds since the epoch the wire delivered where it
    /// does not.
    stored: Value,
}

impl Row {
    /// This row's event id: 36 characters matching `RumEvent$id`'s pattern
    /// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
    fn event_id(&self, index: usize) -> String {
        format!("{index:08x}-1234-4234-8234-123456789012")
    }

    /// This row's event as `PutRumEvents` carries it.
    fn event(&self, index: usize) -> Value {
        json!({
            "id": self.event_id(index),
            "timestamp": self.delivered,
            "type": "com.amazon.rum.page_view_event",
            "details": "{\"pageId\":\"/home\"}",
        })
    }
}

/// The instants every case delivers.
///
/// The bounds of the renderable range are the RFC 3339 grammar's own: `date-fullyear =
/// 4DIGIT` over a calendar whose first year is 0001, so the first instant it can spell is
/// `0001-01-01T00:00:00Z` and the last is `9999-12-31T23:59:59Z`. Each bound is a row, and
/// so is the second just outside it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an ordinary instant",
            delivered: json!(1_704_067_200_i64),
            seconds: 1_704_067_200,
            stored: json!("2024-01-01T00:00:00Z"),
        },
        Row {
            case: "the epoch itself",
            delivered: json!(0_i64),
            seconds: 0,
            stored: json!("1970-01-01T00:00:00Z"),
        },
        Row {
            case: "the second before the epoch",
            delivered: json!(-1_i64),
            seconds: -1,
            stored: json!("1969-12-31T23:59:59Z"),
        },
        Row {
            case: "the first instant RFC 3339 can spell",
            delivered: json!(-62_135_596_800_i64),
            seconds: -62_135_596_800,
            stored: json!("0001-01-01T00:00:00Z"),
        },
        Row {
            case: "one second before the first instant RFC 3339 can spell",
            delivered: json!(-62_135_596_801_i64),
            seconds: -62_135_596_801,
            stored: json!(-62_135_596_801_i64),
        },
        Row {
            case: "the last instant RFC 3339 can spell",
            delivered: json!(253_402_300_799_i64),
            seconds: 253_402_300_799,
            stored: json!("9999-12-31T23:59:59Z"),
        },
        Row {
            case: "one second past the last instant RFC 3339 can spell",
            delivered: json!(253_402_300_800_i64),
            seconds: 253_402_300_800,
            stored: json!(253_402_300_800_i64),
        },
        Row {
            case: "an instant far past every year RFC 3339 can spell",
            delivered: json!(99_999_999_999_999_i64),
            seconds: 99_999_999_999_999,
            stored: json!(99_999_999_999_999_i64),
        },
        Row {
            case: "an instant far before every year RFC 3339 can spell",
            delivered: json!(-99_999_999_999_999_i64),
            seconds: -99_999_999_999_999,
            stored: json!(-99_999_999_999_999_i64),
        },
        Row {
            case: "an unrenderable instant with a subsecond part",
            delivered: json!(99_999_999_999_999.5_f64),
            seconds: 99_999_999_999_999,
            stored: json!(99_999_999_999_999.5_f64),
        },
    ]
}

/// One batch carrying `events`, for the monitor whose minted id is `monitor_id`.
fn batch(batch_id: &str, monitor: &str, monitor_id: &str, events: Vec<Value>) -> Value {
    json!({
        "BatchId": batch_id,
        "AppMonitorDetails": {"name": monitor, "id": monitor_id, "version": "1.0.0"},
        "UserDetails": {
            "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "sessionId": SESSION,
        },
        "RumEvents": Value::Array(events),
    })
}

/// The one session every batch below is collected from, which is also the log stream its
/// copies accumulate in.
const SESSION: &str = "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb";

/// Deliver one batch, asserting the data plane accepted it.
async fn deliver(emulator: &Emulator, case: &str, monitor_id: &str, body: Value) {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/appmonitors/{monitor_id}/"),
            Some(body),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected PutRumEvents to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// The events `GetAppMonitorData` reports for `monitor` over `time_range`, each as the
/// document it is reported as.
async fn collected(emulator: &Emulator, case: &str, monitor: &str, time_range: Value) -> Vec<Value> {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/appmonitor/{monitor}/data"),
            Some(json!({"TimeRange": time_range})),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected GetAppMonitorData to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["Events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            let text = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a reported event must be a JSON string, got {entry}")
            });
            serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a reported event must be a JSON document, got {text:?} ({error})")
            })
        })
        .collect()
}

/// The narrowest window that covers an event at `seconds`.
///
/// Both of `TimeRange`'s bounds are inclusive, so a positive instant is asked for by the
/// single second it falls in. `Before` is modelled `@default(0)` and documented "If you
/// omit this, the time range extends to the time that this operation is performed", so an
/// instant at or before the epoch is asked for by its `After` alone: a `Before` of zero is
/// that default, which stands for now.
fn window_covering(seconds: i64) -> Value {
    if seconds > 0 {
        json!({"After": seconds, "Before": seconds})
    } else {
        json!({"After": seconds})
    }
}

/// A window that starts one second AFTER an event at `seconds`, which therefore covers it
/// nowhere.
fn window_after(seconds: i64) -> Value {
    json!({"After": seconds.saturating_add(1)})
}

#[tokio::test]
async fn every_delivered_instant_is_stored_and_reported_as_the_instant_delivered() {
    let emulator = Emulator::new();
    for (index, row) in rows().iter().enumerate() {
        // One monitor per row, so the window a row asks for selects from its own event
        // only: two rows' instants can be one second apart.
        let monitor = format!("instant-{index}");
        let monitor_id = emulator.create_monitor(&monitor).await;
        deliver(
            &emulator,
            row.case,
            &monitor_id,
            batch(
                "11111111-2222-4333-8444-555555555555",
                &monitor,
                &monitor_id,
                vec![row.event(index)],
            ),
        )
        .await;

        let reported = collected(
            &emulator,
            row.case,
            &monitor,
            window_covering(row.seconds),
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: the window covering the delivered instant must report the one \
             accepted event, got {reported:?}",
            row.case
        );
        assert_eq!(
            reported[0]["id"].as_str(),
            Some(row.event_id(index).as_str()),
            "case {:?}: expected the accepted event to be reported, got {:?}",
            row.case,
            reported[0]
        );
        assert_eq!(
            reported[0]["timestamp"], row.stored,
            "case {:?}: expected the reported document to carry the delivered instant as \
             {:?}, got {:?}",
            row.case, row.stored, reported[0]["timestamp"]
        );

        let outside = collected(&emulator, row.case, &monitor, window_after(row.seconds)).await;
        assert!(
            outside.is_empty(),
            "case {:?}: a window starting after the delivered instant must report nothing, \
             got {outside:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn every_delivered_instant_reaches_the_cloudwatch_logs_copy() {
    let emulator = Emulator::new();
    let monitor = "instant-copies";
    let created = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": monitor,
                "Domain": "example.com",
                "CwLogEnabled": true,
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"setup\": expected CreateAppMonitor to answer 200, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let monitor_id = created.json("setup")["Id"]
        .as_str()
        .expect("CreateAppMonitor must answer an Id")
        .to_string();

    // One batch carrying every row's instant: the copies are one log event per delivered
    // event, in delivery order.
    let rows = rows();
    let events: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| row.event(index))
        .collect();
    deliver(
        &emulator,
        "the copies",
        &monitor_id,
        batch(
            "22222222-2222-4333-8444-555555555555",
            monitor,
            &monitor_id,
            events,
        ),
    )
    .await;

    // The group the copies land in is the one RUM ADVERTISES for this monitor: part of
    // that name is chosen by the service, so it is read off the response rather than
    // rebuilt from the monitor's name.
    let read = emulator
        .rum(ACCOUNT_KEY, "GET", &format!("/appmonitor/{monitor}"), None)
        .await;
    let advertised = read.json("the copies");
    let log_group = advertised["AppMonitor"]["DataStorage"]["CwLog"]["CwLogGroup"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"the copies\": expected GetAppMonitor to advertise a CwLogGroup, got \
                 {advertised}"
            )
        })
        .to_string();

    let (status, body) = emulator
        .logs(
            "GetLogEvents",
            &json!({
                "logGroupName": log_group,
                "logStreamName": SESSION,
                "startFromHead": true,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the copies\": expected GetLogEvents to answer 200, got {status} {body}"
    );
    let copies: Vec<Value> = body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| {
            let message = event["message"].as_str().unwrap_or_else(|| {
                panic!("case \"the copies\": a log event must carry a message, got {event}")
            });
            serde_json::from_str(message).unwrap_or_else(|error| {
                panic!("case \"the copies\": a copy must be a JSON document, got {message:?} ({error})")
            })
        })
        .collect();
    assert_eq!(
        copies.len(),
        rows.len(),
        "case \"the copies\": expected one copy per delivered event, got {copies:?}"
    );
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            copies[index]["id"].as_str(),
            Some(row.event_id(index).as_str()),
            "case {:?}: expected the copies in delivery order, got {:?}",
            row.case,
            copies[index]
        );
        assert_eq!(
            copies[index]["timestamp"], row.stored,
            "case {:?}: expected the copy to carry the delivered instant as {:?}, got {:?}",
            row.case, row.stored, copies[index]["timestamp"]
        );
    }
}
