// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about how this service names a LIST ELEMENT in a refusal.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the interval helpers of the
//! `bandwidth_rate_limit_schedule` suite, so what is asserted is the wire behaviour.
//!
//! The rule under test: a refusal that names WHICH element of a list member it is about
//! numbers that element by its one-based place in the list as the request sent it — the
//! first element is `1` and the last is the list's length — so one service answers "which
//! element did you refuse?" in a single numbering, whichever member the refusal is about and
//! whether the element is named by a validation path (`tags.2.member.value`) or by a
//! sentence that states a rule the model carries only in prose.
//!
//! The tables below span that class rather than one request: every way a bandwidth rate
//! limit schedule can be refused for an element of `BandwidthRateLimitIntervals` (an
//! interval that ends before it starts, an interval naming a day of the week twice, and a
//! pair of intervals that overlap, which names TWO elements), with the offending element at
//! the front, in the middle and at the back of lists of one, two, three and twenty — the
//! published maximum of that member, `@length(min: 0, max: 20)`
//! (`smithy_query(query="shape",shape="BandwidthRateLimitIntervals")`). The last test is the
//! cross-member row: for the same ordinal element, the number the prose refusal states and
//! the number the validation path of a `Tags` refusal carries are the same number.

use serde_json::json;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::ops::types::BandwidthRateLimitInterval;
use backup_gateway::state::BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID;

use crate::bandwidth_rate_limit_schedule::interval;
use crate::gateway_lifecycle::{build_engine, build_router, call, create};
use crate::gateway_server_association::message_of;
use crate::put_bandwidth_rate_limit_schedule::put_schedule;

/// A schedule of `len` intervals that holds together: one per hour of the day on a single
/// day of the week, so no two of them share a minute and none of them is refused. The rows
/// below replace one or two of these with the offending interval(s), so the refusal has to
/// state WHERE in the list the offence sits.
fn holding_list(len: usize) -> Vec<BandwidthRateLimitInterval> {
    (0..len)
        .map(|hour| interval(hour as i32, 0, hour as i32, 59, &[0], Some(51_200)))
        .collect()
}

/// Every element position a refusal's sentence names, in the order it names them: the
/// numbers that follow "position"/"positions", read back out of the message so the
/// assertion is independent of the rest of the wording, which AWS does not publish.
fn named_positions(message: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut words = message.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if !word.starts_with("position") {
            continue;
        }
        while let Some(next) = words.peek() {
            if let Ok(number) = next.trim_end_matches([',', '.', ':']).parse::<usize>() {
                positions.push(number);
                words.next();
            } else if *next == "and" {
                words.next();
            } else {
                break;
            }
        }
    }
    positions
}

/// The element position the validation path inside a constraint clause carries:
/// `<list>.<position>.member.<member>`.
fn position_in_path(message: &str) -> usize {
    let (_, rest) = message
        .split_once(" at '")
        .unwrap_or_else(|| panic!("no constraint clause in {message:?}"));
    let (path, _) = rest
        .split_once("' failed to satisfy constraint: ")
        .unwrap_or_else(|| panic!("no constraint clause in {message:?}"));
    let mut segments = path.split('.');
    segments.next();
    segments
        .next()
        .and_then(|segment| segment.parse().ok())
        .unwrap_or_else(|| panic!("no element position in path {path:?}"))
}

/// One row: the schedule sent, and the element position(s) the refusal must name.
struct Row {
    case: &'static str,
    intervals: Vec<BandwidthRateLimitInterval>,
    expected: Vec<usize>,
}

/// An interval that is no window of a single day — it ends in an earlier hour than it
/// starts — at `position` of a list of `len`.
fn inverted_at(len: usize, position: usize) -> Vec<BandwidthRateLimitInterval> {
    let mut intervals = holding_list(len);
    intervals[position - 1] = interval(22, 0, 6, 0, &[3], Some(51_200));
    intervals
}

/// An interval naming one day of the week twice, at `position` of a list of `len`.
fn repeated_day_at(len: usize, position: usize) -> Vec<BandwidthRateLimitInterval> {
    let mut intervals = holding_list(len);
    intervals[position - 1] = interval(9, 0, 10, 0, &[1, 1], None);
    intervals
}

/// Two intervals sharing a day of the week and a minute of it, at `first` and `second` of a
/// list of `len`. They sit on a day no other interval of the list names, so the pair the
/// refusal is about is exactly this one.
fn overlapping_at(
    len: usize,
    first: usize,
    second: usize,
) -> Vec<BandwidthRateLimitInterval> {
    let mut intervals = holding_list(len);
    intervals[first - 1] = interval(9, 0, 10, 0, &[3], Some(51_200));
    intervals[second - 1] = interval(9, 30, 10, 30, &[3], Some(102_400));
    intervals
}

/// The refusals, spanning each way an element of the schedule is refused and where in the
/// list the offending element sits.
fn rows() -> Vec<Row> {
    let row = |case, intervals, expected| Row {
        case,
        intervals,
        expected,
    };
    vec![
        row(
            "an inverted interval, the only element",
            inverted_at(1, 1),
            vec![1],
        ),
        row(
            "an inverted interval, first of two",
            inverted_at(2, 1),
            vec![1],
        ),
        row(
            "an inverted interval, second of two",
            inverted_at(2, 2),
            vec![2],
        ),
        row(
            "an inverted interval, second of three",
            inverted_at(3, 2),
            vec![2],
        ),
        row(
            "an inverted interval, last of three",
            inverted_at(3, 3),
            vec![3],
        ),
        row(
            "an inverted interval, last of the twenty-element maximum",
            inverted_at(20, 20),
            vec![20],
        ),
        row(
            "a repeated day of the week, the only element",
            repeated_day_at(1, 1),
            vec![1],
        ),
        row(
            "a repeated day of the week, second of two",
            repeated_day_at(2, 2),
            vec![2],
        ),
        row(
            "a repeated day of the week, third of three",
            repeated_day_at(3, 3),
            vec![3],
        ),
        row(
            "a repeated day of the week, last of the twenty-element maximum",
            repeated_day_at(20, 20),
            vec![20],
        ),
        row(
            "an overlapping pair, the first two elements",
            overlapping_at(2, 1, 2),
            vec![1, 2],
        ),
        row(
            "an overlapping pair, the last two of three",
            overlapping_at(3, 2, 3),
            vec![2, 3],
        ),
        row(
            "an overlapping pair straddling an element that holds",
            overlapping_at(3, 1, 3),
            vec![1, 3],
        ),
        row(
            "an overlapping pair, the last two of the twenty-element maximum",
            overlapping_at(20, 19, 20),
            vec![19, 20],
        ),
        row(
            "an overlapping pair, the first and the last of twenty",
            overlapping_at(20, 1, 20),
            vec![1, 20],
        ),
    ]
}

/// A refused schedule names the offending interval by its one-based place in the list the
/// request sent: never `0`, never past the list's length, and the last element of a list of
/// `n` is `n`.
#[tokio::test]
async fn a_refused_schedule_names_the_interval_by_its_one_based_position() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-positions").await;

    for Row {
        case,
        intervals,
        expected,
    } in rows()
    {
        let reply = put_schedule(&router, &arn, &intervals).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply
                    .body
                    .get("ErrorCode")
                    .and_then(serde_json::Value::as_str),
            ),
            (
                400,
                Some("ValidationException"),
                Some(BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID),
            ),
            "case {case:?}: the schedule must be refused with 400 ValidationException and \
             the {BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID:?} ErrorCode, got {} {:?}",
            reply.status,
            reply.body
        );

        let stated = message_of(&reply);
        let named = named_positions(&stated);
        assert_eq!(
            named, expected,
            "case {case:?}: the refusal must name the offending element(s) at position(s) \
             {expected:?}, got {named:?} in {stated:?}"
        );
        assert!(
            named
                .iter()
                .all(|position| (1..=intervals.len()).contains(position)),
            "case {case:?}: every named position must be a place in a list of {} elements, \
             counted from 1, got {named:?} in {stated:?}",
            intervals.len()
        );

        let fragment = match named.as_slice() {
            [only] => format!("at position {only} "),
            [first, second] => format!("at positions {first} and {second} "),
            other => panic!("case {case:?}: unexpected positions {other:?} in {stated:?}"),
        };
        assert!(
            stated.contains(&fragment),
            "case {case:?}: the refusal must state {fragment:?}, got {stated:?}"
        );
    }
}

/// One numbering for the whole service: for the SAME ordinal element of a list member, the
/// number a prose refusal of `BandwidthRateLimitIntervals` states is the number the
/// validation path of a `Tags` refusal carries — and both are the element's place counted
/// from 1.
#[tokio::test]
async fn the_prose_refusal_and_a_validation_path_number_one_element_alike() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-one-numbering").await;

    for ordinal in [1usize, 2, 3] {
        let case = format!("element {ordinal} of {ordinal}");

        // The prose side: the offending interval LAST of a list of `ordinal`.
        let refused_schedule = put_schedule(&router, &arn, &inverted_at(ordinal, ordinal)).await;
        assert_eq!(
            (refused_schedule.status, refused_schedule.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: the schedule must be refused with 400 ValidationException, got \
             {} {:?}",
            refused_schedule.status,
            refused_schedule.body
        );
        let schedule_message = message_of(&refused_schedule);
        let prose = named_positions(&schedule_message);

        // The validation-path side: the offending tag pair LAST of a list of `ordinal`, the
        // value half carrying a space, which no tag value may.
        let mut pairs: Vec<serde_json::Value> = (1..ordinal)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": "fine" }))
            .collect();
        pairs.push(json!({ "Key": "owner", "Value": "two words" }));
        let refused_tags = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": pairs }),
        )
        .await;
        assert_eq!(
            (refused_tags.status, refused_tags.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: the tag pair must be refused with 400 ValidationException, got \
             {} {:?}",
            refused_tags.status,
            refused_tags.body
        );
        let path_position = position_in_path(&message_of(&refused_tags));

        assert_eq!(
            prose,
            vec![path_position],
            "case {case:?}: the prose refusal must number the element the way the \
             validation path does ({path_position}), got {prose:?} in {schedule_message:?}"
        );
        assert_eq!(
            prose,
            vec![ordinal],
            "case {case:?}: the last element of a list of {ordinal} must be numbered \
             {ordinal}, got {prose:?} in {schedule_message:?}"
        );
    }
}
