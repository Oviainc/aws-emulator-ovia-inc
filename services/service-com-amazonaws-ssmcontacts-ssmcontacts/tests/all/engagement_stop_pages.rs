// SPDX-License-Identifier: CC0-1.0

//! What stopping an engagement does to its pages, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` through the harness
//! `tests/all/contact_lifecycle.rs` builds, so what is asserted is what a signing AWS client
//! would see.
//!
//! The rule under test: an engagement ends at the instant its stop records, so every plan
//! stage whose send instant had not arrived by then is never entered and engages no contact
//! channel — the pages of those stages do not exist from then on, so no read reports them and
//! no acknowledgement is recorded against them, while the pages the engagement had already
//! sent, and the pages of every other engagement, are left exactly as they were.
//!
//! It is asserted over two tables, each varying what the rule generalises over:
//!
//! 1. [`SCHEDULE_CASES`] — the plan's own schedule: the stage durations decide which stages
//!    a stop taken right after the start has reached, so the rows span stage counts, both
//!    published bounds of `Stage$DurationInMinutes` and a duration one past its minimum;
//! 2. [`INSTANT_CASES`] — where one page's send instant lies relative to the stop, from an
//!    hour before it to an hour after it, which is the comparison the rule turns on. A row's
//!    offset stays seconds away from the stop rather than milliseconds, so the verdict never
//!    turns on how long the calls between the two instants took.
//!
//! The accept code AWS sends WITH a page is delivered out of band and never appears on the
//! control-plane wire, so — exactly as the acknowledgement suite does — this one reads it
//! from the store.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, PageRecord};

use crate::contact_lifecycle::{error_code, Emulator};
use crate::data_plane_precondition::{create_channel, onboarded_contact};
use crate::page_acknowledgement::{
    channel_target, describe_page, pages_of, set_plan, start_engagement, CHANNELS,
};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// The contact every engagement in this suite names, and the channels its plans target.
struct Fixture {
    contact: String,
    /// One channel per row of `CHANNELS`, in that order.
    channels: Vec<String>,
}

/// Create the onboarded contact and one channel per `CHANNELS` row, so a plan stage can
/// target a channel of every kind the model admits.
async fn fixture(emulator: &Emulator) -> Fixture {
    let contact = onboarded_contact(emulator, "oncall-primary").await;
    let mut channels: Vec<String> = Vec::new();
    for (name, channel_type, address) in CHANNELS {
        channels.push(
            create_channel(
                emulator,
                json!({
                    "ContactId": contact,
                    "Name": name,
                    "Type": channel_type,
                    "DeliveryAddress": {"SimpleAddress": address},
                }),
                name,
            )
            .await,
        );
    }
    Fixture { contact, channels }
}

/// Give `contact` a plan of one stage per entry of `durations`, stage `i` targeting the
/// fixture's channel `i`.
async fn set_staged_plan(
    emulator: &Emulator,
    fixture: &Fixture,
    durations: &[i32],
    case: &str,
) {
    let stages: Vec<Value> = durations
        .iter()
        .enumerate()
        .map(|(index, duration)| {
            json!({
                "DurationInMinutes": duration,
                "Targets": [channel_target(&fixture.channels[index])],
            })
        })
        .collect();
    set_plan(emulator, &fixture.contact, json!({"Stages": stages}), case).await;
}

/// `StopEngagement`, answering the status and the body.
async fn stop_engagement(emulator: &Emulator, engagement_arn: &str, reason: &str) -> (u16, Value) {
    emulator
        .call(
            "StopEngagement",
            json!({"EngagementId": engagement_arn, "Reason": reason}),
        )
        .await
}

/// `AcceptPage` for `page`, with the accept code Incident Manager sent with it.
async fn accept_page(emulator: &Emulator, page: &PageRecord, accept_type: &str) -> (u16, Value) {
    emulator
        .call(
            "AcceptPage",
            json!({
                "PageId": page.arn,
                "AcceptType": accept_type,
                "AcceptCode": page.accept_code,
            }),
        )
        .await
}

/// The `StopTime` `DescribeEngagement` reports for `engagement_arn`, as epoch seconds.
async fn stop_time(emulator: &Emulator, engagement_arn: &str, case: &str) -> f64 {
    let (status, body) = emulator
        .call("DescribeEngagement", json!({"EngagementId": engagement_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeEngagement to answer 200, got {status} {body:?}"
    );
    body.get("StopTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StopTime, got {body:?}"))
}

/// Move the page `page_arn` names to the send instant `at`, as a plan whose stage was due
/// then would have left it.
///
/// Written to the store directly because no modelled call moves a page's send instant — the
/// plan's own schedule does, and a suite cannot wait minutes for it.
fn set_sent_time(emulator: &Emulator, page_arn: &str, at: SystemTime) {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let mut state = state.lock();
    let page = state
        .pages
        .get_mut(page_arn)
        .unwrap_or_else(|| panic!("the store holds no page {page_arn:?}"));
    page.sent_time = at;
}

/// `offset_secs` from now, which a negative value puts in the past.
fn from_now(offset_secs: i64) -> SystemTime {
    let now = SystemTime::now();
    let delta = Duration::from_secs(offset_secs.unsigned_abs());
    if offset_secs.is_negative() {
        now.checked_sub(delta)
    } else {
        now.checked_add(delta)
    }
    .expect("an instant within an hour of now is representable")
}

/// Assert that the page `page` names is gone from every reader a page has: a read reports it
/// as a page nothing sent, and no acknowledgement is recorded against it.
async fn assert_page_cancelled(emulator: &Emulator, page: &PageRecord, case: &str) {
    let (status, body) = describe_page(emulator, &page.arn).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected DescribePage to answer 404 for the page of a stage the stop \
         prevented, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case {case:?}: expected a ResourceNotFoundException from DescribePage, got {body:?}"
    );

    let (status, body) = accept_page(emulator, page, "DELIVERED").await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected AcceptPage to answer 404 for the page of a stage the stop \
         prevented, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case {case:?}: expected a ResourceNotFoundException from AcceptPage, got {body:?}"
    );

    let (status, body) = emulator
        .call("ListPageReceipts", json!({"PageId": page.arn}))
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected ListPageReceipts to answer 404 for the page of a stage the \
         stop prevented, got {status} {body:?}"
    );
}

/// Assert that the page `page` names is still the sent page it was: it is readable, and it
/// still takes the acknowledgement of the contact it reached.
async fn assert_page_sent(emulator: &Emulator, page: &PageRecord, case: &str) {
    let (status, body) = describe_page(emulator, &page.arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribePage to answer 200 for a page the engagement had \
         sent, got {status} {body:?}"
    );
    let (status, body) = accept_page(emulator, page, "DELIVERED").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AcceptPage to answer 200 for a page the engagement had sent, \
         got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. The plan's schedule decides which pages a stop leaves standing
// ---------------------------------------------------------------------------

/// One row of the schedule table.
///
/// `DurationInMinutes` is "the time to wait until beginning the next stage" in the range
/// 0..=30 (model: `Stage$DurationInMinutes`), so a stage is entered once the stages AHEAD of
/// it have had their wait: the stages a stop taken right after the start has reached are
/// exactly those whose durations ahead of them sum to zero.
struct ScheduleCase {
    case: &'static str,
    /// One stage per entry, stage `i` targeting the fixture's channel `i`.
    durations: &'static [i32],
    /// The channel indices whose pages the stop must leave standing, in page order.
    survives: &'static [usize],
}

const SCHEDULE_CASES: &[ScheduleCase] = &[
    ScheduleCase {
        case: "every stage waits the published minimum, so the start reached all three",
        durations: &[0, 0, 0],
        survives: &[0, 1, 2],
    },
    ScheduleCase {
        case: "a plan that waits minutes between stages: only the first was reached",
        durations: &[5, 10, 0],
        survives: &[0],
    },
    ScheduleCase {
        case: "two stages share the start instant and the third waits behind them",
        durations: &[0, 5, 0],
        survives: &[0, 1],
    },
    ScheduleCase {
        case: "one minute past the published minimum is still a stage not reached",
        durations: &[1, 0, 0],
        survives: &[0],
    },
    ScheduleCase {
        case: "the published maximum wait ahead of the later stages",
        durations: &[30, 0, 0],
        survives: &[0],
    },
    ScheduleCase {
        case: "a single stage is reached however long it waits for a stage that follows",
        durations: &[30],
        survives: &[0],
    },
];

#[tokio::test]
async fn a_stop_cancels_the_pages_of_the_stages_it_never_reached() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in SCHEDULE_CASES {
        let case = row.case;
        set_staged_plan(&emulator, &fixture, row.durations, case).await;

        // A second engagement of the same plan, never stopped: the stop below must leave its
        // pages alone, however far ahead their own stages are.
        let running = start_engagement(&emulator, &fixture.contact, case).await;
        let stopped = start_engagement(&emulator, &fixture.contact, case).await;

        let minted = pages_of(&emulator, &stopped);
        assert_eq!(
            minted.len(),
            row.durations.len(),
            "case {case:?}: expected the start to page one channel per stage, got {:?}",
            minted
                .iter()
                .map(|page| page.contact_channel_arn.clone())
                .collect::<Vec<_>>()
        );

        let (status, body) = stop_engagement(&emulator, &stopped, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopEngagement to answer 200, got {status} {body:?}"
        );
        let _ = stop_time(&emulator, &stopped, case).await;

        let standing: Vec<String> = pages_of(&emulator, &stopped)
            .iter()
            .map(|page| page.contact_channel_arn.clone())
            .collect();
        let expected: Vec<String> = row
            .survives
            .iter()
            .map(|index| fixture.channels[*index].clone())
            .collect();
        assert_eq!(
            standing, expected,
            "case {case:?}: expected the stop to leave the pages of the stages it had \
             reached, got {standing:?}"
        );

        for page in &minted {
            if standing.contains(&page.contact_channel_arn) {
                assert_page_sent(&emulator, page, case).await;
            } else {
                assert_page_cancelled(&emulator, page, case).await;
            }
        }

        let untouched = pages_of(&emulator, &running);
        assert_eq!(
            untouched.len(),
            row.durations.len(),
            "case {case:?}: expected the running engagement to keep all {} of its pages, got \
             {:?}",
            row.durations.len(),
            untouched
                .iter()
                .map(|page| page.contact_channel_arn.clone())
                .collect::<Vec<_>>()
        );
        for page in &untouched {
            let (status, body) = describe_page(&emulator, &page.arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribePage to answer 200 for a page of the running \
                 engagement, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. The comparison is the page's send instant against the stop instant
// ---------------------------------------------------------------------------

/// One row of the stop-instant table: where the page's own send instant lies relative to the
/// stop that follows it.
struct InstantCase {
    case: &'static str,
    /// Seconds from the stop; a negative value is a page Incident Manager had already sent.
    offset_secs: i64,
    survives: bool,
}

const INSTANT_CASES: &[InstantCase] = &[
    InstantCase {
        case: "sent an hour before the stop",
        offset_secs: -3600,
        survives: true,
    },
    InstantCase {
        case: "sent a second before the stop",
        offset_secs: -1,
        survives: true,
    },
    InstantCase {
        case: "due seconds after the stop",
        offset_secs: 5,
        survives: false,
    },
    InstantCase {
        case: "due five minutes after the stop",
        offset_secs: 300,
        survives: false,
    },
    InstantCase {
        case: "due an hour after the stop",
        offset_secs: 3600,
        survives: false,
    },
];

#[tokio::test]
async fn a_stop_cancels_exactly_the_pages_not_yet_sent_at_the_stop_instant() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    // One stage, one target: each engagement below sends exactly one page, whose send instant
    // the row then places on one side of the stop.
    set_staged_plan(&emulator, &fixture, &[0], "the one-page plan").await;

    for row in INSTANT_CASES {
        let case = row.case;
        let running = start_engagement(&emulator, &fixture.contact, case).await;
        let stopped = start_engagement(&emulator, &fixture.contact, case).await;

        let page = pages_of(&emulator, &stopped)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the start must have sent one page"));
        let running_page = pages_of(&emulator, &running)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: the start must have sent one page"));
        set_sent_time(&emulator, &page.arn, from_now(row.offset_secs));
        set_sent_time(&emulator, &running_page.arn, from_now(row.offset_secs));

        let (status, body) = stop_engagement(&emulator, &stopped, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopEngagement to answer 200, got {status} {body:?}"
        );

        let standing = pages_of(&emulator, &stopped);
        assert_eq!(
            standing.len(),
            usize::from(row.survives),
            "case {case:?}: expected the page to {}, got {standing:?}",
            if row.survives { "survive" } else { "be cancelled" }
        );
        if row.survives {
            assert_page_sent(&emulator, &page, case).await;
        } else {
            assert_page_cancelled(&emulator, &page, case).await;
        }

        // The stop names ONE engagement, so the page of another engagement due at the very
        // same instant is untouched.
        let (status, body) = describe_page(&emulator, &running_page.arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the running engagement's page to stay readable, got \
             {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. A repeated stop cancels exactly what the first one did
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_stop_cancels_nothing_further_and_moves_no_stop_instant() {
    let case = "a plan whose second and third stages the stop never reached";
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    set_staged_plan(&emulator, &fixture, &[5, 10, 0], case).await;

    let engagement = start_engagement(&emulator, &fixture.contact, case).await;
    let (status, body) = stop_engagement(&emulator, &engagement, case).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first StopEngagement to answer 200, got {status} {body:?}"
    );
    let first_stop = stop_time(&emulator, &engagement, case).await;
    let after_first: Vec<String> = pages_of(&emulator, &engagement)
        .iter()
        .map(|page| page.arn.clone())
        .collect();

    let (status, body) = stop_engagement(&emulator, &engagement, "stopped again").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the second StopEngagement to answer 200, got {status} {body:?}"
    );
    let after_second: Vec<String> = pages_of(&emulator, &engagement)
        .iter()
        .map(|page| page.arn.clone())
        .collect();

    assert_eq!(
        after_second, after_first,
        "case {case:?}: expected the second stop to leave the same pages standing, got \
         {after_second:?}"
    );
    let second_stop = stop_time(&emulator, &engagement, case).await;
    assert_eq!(
        second_stop, first_stop,
        "case {case:?}: expected the stop instant to stay at {first_stop}, got {second_stop}"
    );
}
