// SPDX-License-Identifier: CC0-1.0

//! The idle timeouts this service is configured with, as a POLLING client sees them.
//!
//! An idle timeout is the one configuration here whose effect nothing but the clock
//! produces: no call moves an application to `STOPPED` for having been idle, and no call
//! terminates a session for it either. The rule under test is the one that makes that
//! effect reachable:
//!
//! * the state change the configured idle timeout owes falls due inside the window a
//!   client polls a lifecycle edge in, for the SMALLEST idle timeout the model admits;
//! * it is a span, never zero: the resource is observable in the state it is idle in
//!   before the change falls due;
//! * the configured value ORDERS those changes — a resource configured for more idle
//!   minutes always reaches the change after one configured for fewer, and one
//!   configured not to stop at all never reaches it;
//! * the configured value itself is reported back verbatim, whatever span the emulator
//!   serves it as.
//!
//! Every observation goes through the router over HTTP (`GetApplication`, `GetSession`),
//! the way a polling client makes it, and no state is reached in or rewound through the
//! store: the point of these cases is exactly that a client with no access to the store
//! can see the edge. The spans themselves are asserted over the WHOLE published range of
//! both configurations in [`the_span_an_idle_timeout_is_served_as_orders_the_published_range`].

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::{
    idle_span, MAX_TRANSIENT_SPAN,
};

use super::application_lifecycle::{
    create_application, emulator, get_application, minimal_create, start_application,
};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};

/// How long a client polls for a lifecycle edge before it gives up. A client waiting for
/// a resource to park itself polls for seconds, not for the minutes an idle timeout is
/// stated in, so this is the window the smallest configured idle timeout has to fall due
/// inside.
const CLIENT_PATIENCE: Duration = Duration::from_secs(30);

/// How often that client polls.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// The bounds `AutoStopConfig$idleTimeoutMinutes` publishes, and the default its
/// documentation states, written out here rather than read from `crate::state` so a wrong
/// value in `src/` cannot make this test pass. Source:
/// `smithy_query(query="shape",shape="AutoStopConfig")` — `@range(min: 1, max: 10080)`,
/// "The amount of idle time in minutes after which your application will automatically
/// stop. Defaults to 15 minutes."
const MIN_IDLE_MINUTES: i64 = 1;
const MAX_IDLE_MINUTES: i64 = 10_080;
const DEFAULT_IDLE_MINUTES: i64 = 15;

/// The bounds `StartSessionRequest$idleTimeoutMinutes` publishes, from
/// `smithy_query(query="operation",operation="StartSession")` — `@range(min: 0, max:
/// 1000000)`. Its minimum, zero, names no timeout at all.
const MIN_SESSION_IDLE_MINUTES: i64 = 0;
const MAX_SESSION_IDLE_MINUTES: i64 = 1_000_000;

/// One row of the application table: what the create configured, the idle minutes that
/// configuration leaves in effect, and whether the automatic stop is owed at all.
struct AutoStopRow {
    case: &'static str,
    /// The `autoStopConfiguration` the create carries, or `None` to name none at all.
    configuration: Option<Value>,
    /// The `idleTimeoutMinutes` every read must report for it.
    effective_minutes: i64,
    /// Whether the configuration owes the application an automatic stop.
    stops: bool,
}

/// What a poll of one row has seen so far.
struct Observed {
    application_id: String,
    /// When the row was first reported `STARTED`, and the `updatedAt` of that read.
    started_at: Option<Instant>,
    started_updated_at: f64,
    /// When the row was first reported `STOPPED`, and that whole read.
    stopped_at: Option<Instant>,
    stopped_read: Option<Value>,
}

/// The rows: they differ along the configured idle timeout (both published bounds, the
/// documented default taken by naming no configuration, and a value between them) and
/// along whether the stop is switched on, which is what decides whether the edge is owed
/// at all.
fn auto_stop_rows() -> Vec<AutoStopRow> {
    vec![
        AutoStopRow {
            case: "the published minimum, enabled explicitly",
            configuration: Some(json!({ "enabled": true, "idleTimeoutMinutes": MIN_IDLE_MINUTES })),
            effective_minutes: MIN_IDLE_MINUTES,
            stops: true,
        },
        AutoStopRow {
            case: "twice the minimum, with enabled left to its documented default",
            configuration: Some(json!({ "idleTimeoutMinutes": MIN_IDLE_MINUTES * 2 })),
            effective_minutes: MIN_IDLE_MINUTES * 2,
            stops: true,
        },
        AutoStopRow {
            case: "the documented default, with no configuration at all",
            configuration: None,
            effective_minutes: DEFAULT_IDLE_MINUTES,
            stops: true,
        },
        AutoStopRow {
            case: "the published maximum",
            configuration: Some(json!({ "enabled": true, "idleTimeoutMinutes": MAX_IDLE_MINUTES })),
            effective_minutes: MAX_IDLE_MINUTES,
            stops: true,
        },
        AutoStopRow {
            case: "the minimum idle timeout with the stop switched off",
            configuration: Some(
                json!({ "enabled": false, "idleTimeoutMinutes": MIN_IDLE_MINUTES }),
            ),
            effective_minutes: MIN_IDLE_MINUTES,
            stops: false,
        },
    ]
}

/// An `Application$state` read over HTTP.
async fn application_state(router: &MultiServiceRouter, id: &str) -> Value {
    get_application(router, id).await.at("/application")
}

/// A `STARTED` application per row, with nothing running on it: every one of them is
/// idle from the instant it came up.
async fn started_rows(router: &MultiServiceRouter, rows: &[AutoStopRow]) -> Vec<Observed> {
    let mut observed = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut request = minimal_create(&format!("tok-idle-window-{index}"));
        if let Some(configuration) = row.configuration.clone() {
            request["autoStopConfiguration"] = configuration;
        }
        let created = create_application(router, request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateApplication expected 200, got {} with {:?}",
            row.case, created.status, created.body
        );
        let application_id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let started = start_application(router, &application_id).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: StartApplication expected 200, got {} with {:?}",
            row.case, started.status, started.body
        );
        observed.push(Observed {
            application_id,
            started_at: None,
            started_updated_at: 0.0,
            stopped_at: None,
            stopped_read: None,
        });
    }
    observed
}

/// Rule: the automatic stop an `autoStopConfiguration` owes an idle application is
/// reachable by a client that polls `GetApplication` — the smallest idle timeout the
/// model admits falls due inside a client's polling window, every application is
/// observable `STARTED` first, a configuration naming more idle minutes stops later than
/// one naming fewer, and a configuration with the stop switched off never stops at all.
/// No `StopApplication` is called anywhere in this case.
#[tokio::test]
async fn an_idle_application_reaches_stopped_inside_a_client_polling_window() {
    let router = emulator();
    let rows = auto_stop_rows();
    let mut observed = started_rows(&router, &rows).await;

    // Poll every row the way a client does, until the two rows with the shortest idle
    // timeouts have both parked themselves or the client's patience runs out.
    let polling_started = Instant::now();
    loop {
        for (row, state) in rows.iter().zip(observed.iter_mut()) {
            let application = application_state(&router, &state.application_id).await;
            let reported = application
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // The configured value is reported verbatim on every read, whatever span the
            // emulator serves it as.
            assert_eq!(
                application
                    .pointer("/autoStopConfiguration/idleTimeoutMinutes")
                    .and_then(Value::as_i64),
                Some(row.effective_minutes),
                "case {:?}: every read must report the configured idleTimeoutMinutes \
                 {}, got {:?}",
                row.case,
                row.effective_minutes,
                application.pointer("/autoStopConfiguration/idleTimeoutMinutes")
            );
            match reported.as_str() {
                "STARTED" => {
                    assert!(
                        state.stopped_at.is_none(),
                        "case {:?}: an application that stopped itself must stay stopped, \
                         got STARTED after STOPPED",
                        row.case
                    );
                    if state.started_at.is_none() {
                        state.started_at = Some(Instant::now());
                        state.started_updated_at = application
                            .get("updatedAt")
                            .and_then(Value::as_f64)
                            .unwrap_or_default();
                    }
                }
                "STOPPED" => {
                    assert!(
                        state.started_at.is_some(),
                        "case {:?}: the idle span is a span, so the application must be \
                         observable STARTED before it stops itself",
                        row.case
                    );
                    if state.stopped_at.is_none() {
                        state.stopped_at = Some(Instant::now());
                        state.stopped_read = Some(application.clone());
                    }
                }
                "STARTING" => {}
                other => panic!(
                    "case {:?}: an application nothing stopped is STARTING, STARTED or \
                     STOPPED, got {other:?}",
                    row.case
                ),
            }
        }
        let two_shortest_parked = observed
            .iter()
            .take(2)
            .all(|state| state.stopped_at.is_some());
        if two_shortest_parked || polling_started.elapsed() >= CLIENT_PATIENCE {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // The published minimum falls due inside the window a client polls in, and so does
    // the row at twice the minimum.
    for (row, state) in rows.iter().zip(observed.iter()).take(2) {
        let stopped_at = state.stopped_at.unwrap_or_else(|| {
            panic!(
                "case {:?}: an application idle past {} configured minute(s) must reach \
                 STOPPED inside {CLIENT_PATIENCE:?}, got no such observation in {:?}",
                row.case,
                row.effective_minutes,
                polling_started.elapsed()
            )
        });
        let read = state.stopped_read.clone().unwrap_or_default();
        let details = read
            .get("stateDetails")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            details.contains("idle"),
            "case {:?}: the automatic stop must say it was idle, got {:?}",
            row.case,
            read.get("stateDetails")
        );
        let stopped_updated_at = read.get("updatedAt").and_then(Value::as_f64).unwrap_or(0.0);
        assert!(
            stopped_updated_at > state.started_updated_at,
            "case {:?}: the stop moved updatedAt forward, got {stopped_updated_at} after \
             {}",
            row.case,
            state.started_updated_at
        );
        assert!(
            state
                .started_at
                .is_some_and(|started| stopped_at > started),
            "case {:?}: the stop must be observed after the start, got {stopped_at:?} \
             against {:?}",
            row.case,
            state.started_at
        );
    }

    // The configured value orders the edges: fewer idle minutes stop earlier, more idle
    // minutes have not stopped yet, and the row with the stop switched off never stops.
    let minimum_stopped = observed[0]
        .stopped_at
        .expect("the published minimum stopped inside the window");
    let twice_stopped = observed[1]
        .stopped_at
        .expect("twice the minimum stopped inside the window");
    assert!(
        twice_stopped > minimum_stopped,
        "twice the minimum idle timeout must stop after the minimum, got {twice_stopped:?} \
         against {minimum_stopped:?}"
    );
    for (row, state) in rows.iter().zip(observed.iter()).skip(2) {
        assert!(
            state.stopped_at.is_none(),
            "case {:?}: {} configured idle minutes, with the stop {}, must not stop \
             before {} minute(s) does — got a stop at {:?}",
            row.case,
            row.effective_minutes,
            if row.stops { "on" } else { "off" },
            MIN_IDLE_MINUTES * 2,
            state.stopped_at
        );
        let state_now = application_state(&router, &state.application_id).await;
        assert_eq!(
            state_now.get("state").and_then(Value::as_str),
            Some("STARTED"),
            "case {:?}: it must still be STARTED, got {:?}",
            row.case,
            state_now.get("state")
        );
    }
}

/// The same rule on the other idle timeout this service is configured with: a session's
/// `idleTimeoutMinutes` ends it, and the end is reachable by a client polling
/// `GetSession` — the smallest timeout the model admits past zero falls due inside the
/// polling window, a longer one has not fallen due by then, and the zero the range's
/// minimum names owes no end at all.
#[tokio::test]
async fn an_idle_session_reaches_terminated_inside_a_client_polling_window() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-idle-session").await;

    // (case, the idleTimeoutMinutes the start asks for, whether a termination is owed)
    let rows: Vec<(&str, i64, bool)> = vec![
        ("the smallest timeout past the range's minimum", 1, true),
        ("twice that timeout", 2, true),
        (
            "the range's minimum, which names no timeout",
            MIN_SESSION_IDLE_MINUTES,
            false,
        ),
    ];

    let mut session_ids = Vec::new();
    for (index, (case, minutes, _)) in rows.iter().enumerate() {
        let mut body = minimal_session(&format!("ses-idle-{index}"));
        body["idleTimeoutMinutes"] = json!(minutes);
        let started = start_session(&router, &application_id, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        session_ids.push(
            started
                .at("/sessionId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    let polling_started = Instant::now();
    let mut terminated_at: Vec<Option<Instant>> = rows.iter().map(|_| None).collect();
    let mut terminated_read: Vec<Value> = rows.iter().map(|_| Value::Null).collect();
    loop {
        for (index, (case, minutes, _)) in rows.iter().enumerate() {
            let read = get_session(&router, &application_id, &session_ids[index]).await;
            let session = read.at("/session");
            assert_eq!(
                session.get("idleTimeoutMinutes").and_then(Value::as_i64),
                Some(*minutes),
                "case {case:?}: every read must report the configured idleTimeoutMinutes \
                 {minutes}, got {:?}",
                session.get("idleTimeoutMinutes")
            );
            if session.get("state").and_then(Value::as_str) == Some("TERMINATED")
                && terminated_at[index].is_none()
            {
                terminated_at[index] = Some(Instant::now());
                terminated_read[index] = session.clone();
            }
        }
        if terminated_at[0].is_some() || polling_started.elapsed() >= CLIENT_PATIENCE {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let (case, minutes, _) = rows[0];
    assert!(
        terminated_at[0].is_some(),
        "case {case:?}: a session idle past {minutes} configured minute(s) must reach \
         TERMINATED inside {CLIENT_PATIENCE:?}, got no such observation in {:?}",
        polling_started.elapsed()
    );
    let details = terminated_read[0]
        .get("stateDetails")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        details.contains("idle"),
        "case {case:?}: the automatic termination must say it was idle, got {:?}",
        terminated_read[0].get("stateDetails")
    );

    for (index, (case, minutes, owed)) in rows.iter().enumerate().skip(1) {
        assert!(
            terminated_at[index].is_none(),
            "case {case:?}: {minutes} configured idle minutes, with a termination {}, \
             must not end before the shortest timeout does — got an end at {:?}",
            if *owed { "owed" } else { "not owed" },
            terminated_at[index]
        );
        let state = get_session(&router, &application_id, &session_ids[index])
            .await
            .at("/session/state");
        assert_eq!(
            state,
            json!("STARTED"),
            "case {case:?}: it must still be STARTED, got {state:?}"
        );
    }
}

/// The span the emulator serves a configured idle timeout as, over the WHOLE published
/// range of both configurations: it is never zero, it never shrinks as the configured
/// minutes grow (and grows strictly with them), the smallest value either range admits
/// past zero falls due inside a client's polling window, and the largest stays inside the
/// longest transient span this service ever waits out.
///
/// Exhaustive over `AutoStopConfig$idleTimeoutMinutes`'s `@range(min: 1, max: 10080)`,
/// and over the bounds of `StartSessionRequest$idleTimeoutMinutes`'s
/// `@range(min: 0, max: 1000000)`.
#[test]
fn the_span_an_idle_timeout_is_served_as_orders_the_published_range() {
    assert_eq!(
        idle_span(u64::try_from(MIN_SESSION_IDLE_MINUTES).unwrap_or(0)),
        Duration::ZERO,
        "no configured idle minutes is no idle span, got {:?}",
        idle_span(0)
    );

    let smallest = idle_span(u64::try_from(MIN_IDLE_MINUTES).unwrap_or(1));
    assert!(
        smallest > Duration::ZERO,
        "case {MIN_IDLE_MINUTES}: the smallest configured idle timeout is a span, not an \
         instant, got {smallest:?}"
    );
    assert!(
        smallest < CLIENT_PATIENCE,
        "case {MIN_IDLE_MINUTES}: the smallest configured idle timeout must fall due \
         inside a client's polling window {CLIENT_PATIENCE:?}, got {smallest:?}"
    );

    let mut previous = smallest;
    for minutes in (MIN_IDLE_MINUTES + 1)..=MAX_IDLE_MINUTES {
        let span = idle_span(u64::try_from(minutes).unwrap_or(1));
        assert!(
            span > previous,
            "case {minutes}: more configured idle minutes must be a longer span, got \
             {span:?} after {previous:?}"
        );
        previous = span;
    }

    for minutes in [MAX_IDLE_MINUTES, MAX_SESSION_IDLE_MINUTES] {
        let span = idle_span(u64::try_from(minutes).unwrap_or(1));
        assert!(
            span <= MAX_TRANSIENT_SPAN,
            "case {minutes}: the largest configured idle timeout must stay inside \
             {MAX_TRANSIENT_SPAN:?}, got {span:?}"
        );
    }
}
