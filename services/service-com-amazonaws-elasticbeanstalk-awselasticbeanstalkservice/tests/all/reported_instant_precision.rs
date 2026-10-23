// SPDX-License-Identifier: CC0-1.0

//! The precision of the instants this service reports.
//!
//! The rule under test: a timestamp this service reports names exactly the instant
//! it compares that record by, so a window bound a client copies out of a reported
//! timestamp selects exactly the records the report dates at or after it. Two
//! renderings that differ must therefore order as two distinct instants, and two
//! renderings that are equal must compare equal.
//!
//! Both halves are asserted: the clock helper every record is dated from
//! (`state::now`) over a bounded sample of instants taken a few nanoseconds apart,
//! and `DescribeEvents` over the wire, where every reported `EventDate` of an
//! activity is used in turn as the `StartTime` bound.
//!
//! Why this matters: `aws_smithy_types::DateTime` is compared as `f64` seconds,
//! which resolve about 240 ns at the current epoch, while the `date-time` rendering
//! the query protocol reports carries microseconds. An instant kept at a finer
//! precision than it is reported at therefore renders a whole microsecond away from
//! an instant it compares EQUAL to, and a bound copied from the newer of the two
//! then also admits the older one.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use super::harness::{Answer, Emulator};

/// How many instants the clock sample below draws. Bounded, so the suite stays a
/// test rather than a benchmark; the samples land a few nanoseconds apart, which is
/// the interval the rule is about.
const SAMPLE_COUNT: usize = 500;

/// A timestamp as the query protocol reports it.
fn rendered(at: &DateTime, case: &str) -> String {
    at.fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {at:?} is not renderable: {error}"))
}

/// A reported timestamp read back as the instant it names.
fn parsed(text: &str, case: &str) -> DateTime {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {text:?} is not a timestamp: {error}"))
}

/// Every instant the service's clock mints survives the round trip through the
/// rendering it is reported in, and two instants order the same way as their
/// renderings do.
#[test]
fn a_reported_instant_round_trips_and_orders_as_the_instant_it_renders() {
    let mut previous: Option<DateTime> = None;
    for index in 0..SAMPLE_COUNT {
        let case = format!("sample {index}");
        let at = eb::state::now();
        let text = rendered(&at, &case);
        let back = parsed(&text, &case);

        assert_eq!(
            (back.secs(), back.subsec_nanos()),
            (at.secs(), at.subsec_nanos()),
            "case {case:?}: {text:?} must name the instant it was rendered from, \
             got {}s+{}ns back from {}s+{}ns",
            back.secs(),
            back.subsec_nanos(),
            at.secs(),
            at.subsec_nanos()
        );

        if let Some(earlier) = previous {
            let earlier_text = rendered(&earlier, &case);
            let (earlier_secs, at_secs) = (earlier.as_secs_f64(), at.as_secs_f64());
            if earlier_text == text {
                assert_eq!(
                    earlier_secs, at_secs,
                    "case {case:?}: two instants rendered alike ({text:?}) must compare equal, \
                     got {earlier_secs} and {at_secs}"
                );
            } else {
                assert!(
                    earlier_secs < at_secs,
                    "case {case:?}: {earlier_text:?} renders before {text:?}, so it must compare \
                     before it, got {earlier_secs} and {at_secs}"
                );
            }
        }
        previous = Some(at);
    }
}

/// Every `EventDate` in `answer`, in the order the answer lists them.
fn event_dates(answer: &Answer) -> Vec<String> {
    answer
        .elements_at("Events/member")
        .iter()
        .map(|event| event.value("EventDate").unwrap_or_default().to_string())
        .collect()
}

/// `DescribeEvents` under `StartTime` answers exactly the events its own report
/// dates at or after that instant — for every `EventDate` the report carries,
/// including the events of one activity, which land microseconds apart.
#[tokio::test]
async fn a_start_time_copied_from_a_reported_event_date_selects_exactly_the_reported_events() {
    let emulator = Emulator::new();

    // Several activities, so the log carries events that are microseconds apart
    // (within one activity) as well as milliseconds apart (across activities).
    for name in ["precision-app-a", "precision-app-b", "precision-app-c"] {
        let created = emulator
            .call("CreateApplication", &[("ApplicationName", name)])
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateApplication must answer 200, got {} and body {}",
            created.status, created.body
        );
    }

    let all = emulator.call("DescribeEvents", &[]).await;
    assert_eq!(
        all.status, 200,
        "DescribeEvents must answer 200, got {} and body {}",
        all.status, all.body
    );
    let dates = event_dates(&all);
    assert!(
        !dates.is_empty(),
        "the activities above must have recorded events, got body {}",
        all.body
    );

    // Each row is one reported `EventDate` used as the inclusive lower bound.
    for bound in &dates {
        let expected: Vec<String> = dates
            .iter()
            .filter(|date| {
                let (at, start) = (parsed(date, bound), parsed(bound, bound));
                at.as_secs_f64() >= start.as_secs_f64()
            })
            .cloned()
            .collect();
        let answer = emulator
            .call("DescribeEvents", &[("StartTime", bound.as_str())])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {bound:?}: DescribeEvents must answer 200, got {} and body {}",
            answer.status, answer.body
        );
        let selected = event_dates(&answer);
        assert_eq!(
            selected, expected,
            "case {bound:?}: StartTime selects the events reported at or after it, \
             expected {expected:?}, got {selected:?}"
        );
    }
}
