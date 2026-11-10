// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentEvents` paged over a history that is MUTATED between two pages.
//!
//! The rule under test: a pagination token names the item it was issued for by an
//! identity this service never reassigns, so the token either resumes immediately after
//! that item or — once the item is gone from the result set — is refused. A walk is
//! therefore never silently incomplete: it cannot jump past items that are still held.
//!
//! A deployment's event history is the one result set in this service that the service
//! itself rewrites: `Deployment::append_event` drops the OLDEST events once the log is
//! over `state::MAX_EVENTS_PER_DEPLOYMENT`, which renumbers every surviving event's
//! POSITION. The rows below span that: a log that does not move between pages, one that
//! grows without reaching its ceiling, one trimmed while the event the token names
//! survives, and one trimmed past that event.
//!
//! Driving the ceiling through `UpdateDeployment` alone would cost one clock-owed update
//! cycle per two events, so the history is grown on the STORED record through
//! `Deployment::append_event` — the one path every appender (handler and clock-owed step
//! alike) goes through — exactly as `tests/all/deployment_update.rs` does, and every
//! page is then read over the wire like any other request.

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    service_state, DeploymentEvent, EVENT_IN_PROGRESS, INVALID_NEXT_TOKEN_MESSAGE,
    MAX_EVENTS_PER_DEPLOYMENT,
};

use crate::deployment_lifecycle::create;
use crate::deployment_update::{error_message, one_pattern_per_workload};
use crate::harness::{one_page, Harness};

/// Long enough for every clock-owed step a CREATE schedules to come due (the three
/// `state::TRANSITION_SPAN`s a create walks, plus slack), so the history has stopped
/// growing on its own before a row pages over it. The emulator never sleeps; this is the
/// test waiting for the clock.
const CREATE_SETTLE: Duration = Duration::from_millis(2_100);

/// One way a history can move between two pages of one walk.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    /// Events appended before the walk starts, on top of the ones the create left.
    seeded: usize,
    /// `maxResults` both pages of the walk ask for.
    page_size: usize,
    /// Events appended BETWEEN the first page and the second.
    appended_between: usize,
}

/// The rows: the history is unmoved, grown below the ceiling, trimmed while the event
/// the token names survives, and trimmed past that event.
const ROWS: [Row; 5] = [
    Row {
        label: "a history nothing appends to between pages",
        seeded: 6,
        page_size: 2,
        appended_between: 0,
    },
    Row {
        label: "a history grown between pages but still under its ceiling",
        seeded: 6,
        page_size: 2,
        appended_between: 5,
    },
    Row {
        label: "a history at its ceiling, trimmed between pages, token's event kept",
        seeded: MAX_EVENTS_PER_DEPLOYMENT + 16,
        page_size: 100,
        appended_between: 10,
    },
    Row {
        label: "a history at its ceiling, trimmed by one past the token's own event",
        seeded: MAX_EVENTS_PER_DEPLOYMENT + 16,
        page_size: 1,
        appended_between: 1,
    },
    Row {
        label: "a history at its ceiling, trimmed well past the token's own event",
        seeded: MAX_EVENTS_PER_DEPLOYMENT + 16,
        page_size: 3,
        appended_between: 40,
    },
];

/// One page of `ListDeploymentEvents`: the event descriptions it reported, oldest
/// first, and the token it handed back.
///
/// The page itself is read through the shared single-page helper, so this suite and the
/// others that assert the token contract cannot disagree about how a page is read; what
/// belongs to this suite is projecting each event onto its description.
async fn page(
    harness: &Harness,
    case: &str,
    id: &str,
    page_size: usize,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut request = Map::new();
    request.insert("deploymentId".to_string(), json!(id));
    request.insert("maxResults".to_string(), json!(page_size));
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    let (events, next) = one_page(
        harness,
        case,
        "/listDeploymentEvents",
        Value::Object(request),
        "deploymentEvents",
    )
    .await;
    let described = events
        .iter()
        .map(|event| {
            event["description"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an event carries no description: {event}"))
                .to_string()
        })
        .collect();
    (described, next)
}

/// Append `count` events to the stored history of `id`, each with a description of its
/// own so a page can be matched back to the appends that produced it, and report those
/// descriptions in append order.
fn append_to_history(
    harness: &Harness,
    case: &str,
    id: &str,
    marker: &str,
    count: usize,
) -> Vec<String> {
    let state = service_state(&**harness.engine()).expect("the service's own state store");
    let mut store = state.lock();
    let deployment = store
        .deployments
        .get_mut(id)
        .unwrap_or_else(|| panic!("case {case:?}: the created deployment must be stored"));
    let newest = deployment
        .events
        .last()
        .map(|event| event.timestamp)
        .unwrap_or_else(SystemTime::now);
    let mut described = Vec::with_capacity(count);
    for index in 0..count {
        let description = format!("{marker} {index}");
        deployment.append_event(DeploymentEvent::new(
            "DeploymentUpdateInProgress",
            description.clone(),
            EVENT_IN_PROGRESS,
            newest + Duration::from_millis(index as u64 + 1),
        ));
        described.push(description);
    }
    described
}

/// The descriptions a history of `appended` appends still holds, oldest first: the
/// ceiling keeps the most recent [`MAX_EVENTS_PER_DEPLOYMENT`] of them.
fn kept(appended: &[String]) -> &[String] {
    &appended[appended.len().saturating_sub(MAX_EVENTS_PER_DEPLOYMENT)..]
}

/// How many of `appended`'s appends the ceiling has dropped.
fn dropped(appended: &[String]) -> usize {
    appended.len().saturating_sub(MAX_EVENTS_PER_DEPLOYMENT)
}

#[tokio::test]
async fn a_page_token_resumes_after_its_own_event_however_the_history_moved() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");

    // Every row's deployment is created first and the clock-owed create steps are waited
    // out ONCE, so the history a row pages over only moves when the row moves it.
    let mut rows = Vec::new();
    for (index, row) in ROWS.into_iter().enumerate() {
        let case = format!("{}/{} {}", workload, pattern, row.label);
        let id = create(
            &harness,
            &case,
            &workload,
            &pattern,
            &format!("event-cursor-{index}"),
        )
        .await;
        rows.push((row, case, id));
    }
    std::thread::sleep(CREATE_SETTLE);

    for (row, case, id) in rows {
        // The history as the create left it, in append order: read over the wire, so no
        // event name or description in this suite is written by hand.
        let (created, _) = page(&harness, &case, &id, 100, None).await;
        assert!(
            !created.is_empty(),
            "case {case:?}: a created deployment must have a history, got none"
        );
        let mut appended = created.clone();
        appended.extend(append_to_history(
            &harness, &case, &id, "before the walk", row.seeded,
        ));

        // Page one: the oldest events the history still holds.
        let (first, token) = page(&harness, &case, &id, row.page_size, None).await;
        let expected_first: Vec<String> = kept(&appended).iter().take(row.page_size).cloned().collect();
        assert_eq!(
            first, expected_first,
            "case {case:?}: the first page must report the oldest kept events, got {first:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a history longer than one page must hand back a token")
        });
        let resumed_after = first
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: the first page must not be empty"))
            .clone();
        // Where in the WHOLE append history the token's event sits — the identity the
        // token has to keep naming, whatever the history does next.
        let cursor = appended
            .iter()
            .position(|description| *description == resumed_after)
            .unwrap_or_else(|| panic!("case {case:?}: the paged event {resumed_after:?} was never appended"));

        // Now move the history underneath the walk.
        appended.extend(append_to_history(
            &harness,
            &case,
            &id,
            "between the pages",
            row.appended_between,
        ));

        if cursor < dropped(&appended) {
            // The event the token names is gone from the result set, so the token is
            // refused — the walk cannot silently resume somewhere else.
            let body = harness
                .refused(
                    &case,
                    "/listDeploymentEvents",
                    json!({"deploymentId": id, "maxResults": row.page_size, "nextToken": token}),
                    400,
                    "ValidationException",
                )
                .await;
            let message = error_message(&body);
            assert_eq!(
                message, INVALID_NEXT_TOKEN_MESSAGE,
                "case {case:?}: expected the fixed pagination refusal, got {message:?}"
            );
            assert!(
                !message.contains(token.as_str()),
                "case {case:?}: the refusal echoes the rejected token, got {message:?}"
            );
            continue;
        }

        // The event the token names is still held, so the next page starts at the event
        // appended right after it — not at whatever has since moved into that position.
        let (second, _) = page(&harness, &case, &id, row.page_size, Some(&token)).await;
        let expected_second: Vec<String> = appended
            .iter()
            .skip(cursor + 1)
            .take(row.page_size)
            .cloned()
            .collect();
        assert_eq!(
            second, expected_second,
            "case {case:?}: the second page must resume at the event appended after \
             {resumed_after:?}, got {second:?}"
        );
        // The two pages together are one contiguous run of the append history: nothing
        // between them was skipped and nothing was reported twice.
        let walked: Vec<String> = first.iter().chain(second.iter()).cloned().collect();
        let run: Vec<String> = appended
            .iter()
            .skip(cursor + 1 - first.len())
            .take(walked.len())
            .cloned()
            .collect();
        assert_eq!(
            walked, run,
            "case {case:?}: the walk must be contiguous in append order, got {walked:?}"
        );
    }
}
