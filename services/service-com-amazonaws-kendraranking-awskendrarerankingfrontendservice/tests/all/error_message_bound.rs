// SPDX-License-Identifier: CC0-1.0

//! One rule: a message this service answers an error with fits the modelled length of
//! the `Message` member that carries it, however many positions the request offended in.
//!
//! Every modelled error of this service carries one `Message` member targeting
//! `ErrorMessage`, which the model bounds at `@length(min: 1, max: 2048)`
//! (`smithy_query(query="shape",shape="ErrorMessage")`), while the number of offending
//! positions a single well-formed request can carry is the caller's: `TagList` and
//! `TagKeyList` are `@length 0..200`, so one request offers up to 200 (`TagKeys`) or 400
//! (a `Tag`'s key AND value) positions. The report therefore states the LEADING clauses
//! that fit and states the count of exactly those, so the sentence stays true of the
//! report carrying it.
//!
//! The tables vary what the rule generalises over: the number of offending positions
//! (from one, through the count that first passes the bound, to the modelled maximum),
//! which member of a tag offends, the operation the request reaches (both tag writers
//! and the key list of `UntagResource`), the clause WIDTH and whether its characters are
//! multi-byte (the bound counts characters, not bytes), and the path the refusal takes —
//! a rule a handler states, and a modelled constraint the boundary validator refuses
//! before any handler runs.

use serde_json::{json, Value};

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, create_plan_with_tags, emulator, error_code, error_message};

/// The text a report puts between a clause's position and the requirement it failed, as
/// `state::clause` spells it. Counting it is how a test counts the clauses a report
/// states without re-deriving any of their wording.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// What a report puts between two clauses.
const CLAUSE_SEPARATOR: &str = "; ";

/// How many clauses `message` states.
fn clauses_listed(message: &str) -> usize {
    message.matches(CONSTRAINT_SEPARATOR).count()
}

/// The violation count `message`'s opening sentence states ("`N` validation error[s]
/// detected: …"), or `None` when it opens with no such count.
fn count_stated(message: &str) -> Option<usize> {
    message
        .split_once(" validation error")
        .and_then(|(count, _)| count.parse().ok())
}

/// Assert `message` obeys the rule: it fits the modelled `ErrorMessage` length, states at
/// least one clause, states no more clauses than the request offended positions in, and
/// states a count equal to the number of clauses it carries.
///
/// `offending_positions` is how many clauses the request would produce unbounded. When
/// the report cannot carry them all, it must be PACKED — one more clause of the width the
/// last one has would not have fitted — so a bounded report is still the longest true
/// report the member can hold.
fn assert_bounded_report(case: &str, message: &str, offending_positions: usize) {
    let chars = message.chars().count();
    assert!(
        chars <= state::MAX_ERROR_MESSAGE_CHARS,
        "case {case:?}: expected at most {} characters, got {chars}: {message:?}",
        state::MAX_ERROR_MESSAGE_CHARS
    );
    let listed = clauses_listed(message);
    assert!(
        (1..=offending_positions).contains(&listed),
        "case {case:?}: expected between 1 and {offending_positions} clauses, got {listed}: \
         {message:?}"
    );
    assert_eq!(
        count_stated(message),
        Some(listed),
        "case {case:?}: the stated count must equal the {listed} clauses carried, got \
         {message:?}"
    );
    if listed < offending_positions && listed > 1 {
        let last = message
            .rsplit(CLAUSE_SEPARATOR)
            .next()
            .unwrap_or_default()
            .chars()
            .count();
        assert!(
            chars + CLAUSE_SEPARATOR.chars().count() + last > state::MAX_ERROR_MESSAGE_CHARS,
            "case {case:?}: a shortened report must be packed — one more {last}-character \
             clause past {chars} would still fit under {}: {message:?}",
            state::MAX_ERROR_MESSAGE_CHARS
        );
    }
}

/// `count` clauses, each `width` characters wide, built out of `filler` and the crate's
/// own clause renderer so the shape is the one the service reports.
fn clauses(count: usize, width: usize, filler: char) -> Vec<String> {
    (0..count)
        .map(|index| {
            let member = format!("tags.{}.member.key", index + 1);
            let skeleton = state::clause(&member, "").chars().count();
            let requirement: String = std::iter::repeat_n(filler, width.saturating_sub(skeleton))
                .collect();
            state::clause(&member, &requirement)
        })
        .collect()
}

#[test]
fn a_validation_report_fits_the_modelled_message_length() {
    // The counts span one clause, the handful an existing suite pins, the count at which
    // a report of ~155-character clauses first passes 2048, the modelled `TagList`
    // maximum and the 400 positions a full `TagResource` request offers. The widths span
    // a terse requirement, the width of the character-set requirement this crate reports,
    // and the widest clause the framework can produce (it elides an echoed value at 256
    // characters).
    for count in [0, 1, 2, 3, 13, 14, 15, 100, 200, 400] {
        for (filler_case, filler) in [("ASCII", 'x'), ("multi-byte", '\u{691c}')] {
            for width in [40, 155, 300] {
                let case = format!("{count} clauses of {width} {filler_case} characters");
                let built = clauses(count, width, filler);
                let message = state::validation_report(&built);
                let chars = message.chars().count();
                assert!(
                    chars <= state::MAX_ERROR_MESSAGE_CHARS,
                    "case {case:?}: expected at most {} characters, got {chars}: {message:?}",
                    state::MAX_ERROR_MESSAGE_CHARS
                );
                let listed = clauses_listed(&message);
                assert_eq!(
                    count_stated(&message),
                    Some(listed),
                    "case {case:?}: the stated count must equal the {listed} clauses carried, \
                     got {message:?}"
                );
                if count == 0 {
                    assert_eq!(
                        listed, 0,
                        "case {case:?}: expected no clause, got {message:?}"
                    );
                    continue;
                }
                assert!(
                    listed >= 1,
                    "case {case:?}: expected at least one clause, got {message:?}"
                );
                // The report keeps request order: it starts at the first offending
                // position and carries a prefix of the clauses, never a sample of them.
                let expected_prefix = built[..listed].join(CLAUSE_SEPARATOR);
                assert!(
                    message.ends_with(&expected_prefix),
                    "case {case:?}: expected the first {listed} clauses in request order, got \
                     {message:?}"
                );
            }
        }
    }
}

#[test]
fn one_clause_too_long_to_fit_is_still_stated_whole() {
    // `ErrorMessage` is modelled `min: 1`, so a report stating NO clause is as far
    // outside the shape as one past the maximum: a lone clause is reported whole even
    // where it alone passes the bound. No clause the service builds is this wide — the
    // framework elides an echoed value at 256 characters — so this pins the floor, not a
    // reachable response.
    let built = clauses(1, state::MAX_ERROR_MESSAGE_CHARS + 500, 'x');
    let message = state::validation_report(&built);
    assert_eq!(
        clauses_listed(&message),
        1,
        "a lone clause must be stated: got {message:?}"
    );
    assert_eq!(
        message,
        format!("1 validation error detected: {}", built[0]),
        "a lone clause must be stated whole"
    );
}

/// `count` tags whose key and value are both outside the documented tag character set
/// (`!` is none of Unicode letters, digits, white space or the symbols `_ . : / = + - @`),
/// so each tag offends in TWO positions.
fn illegal_tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("bad!{index}"), "Value": format!("also!{index}") }))
            .collect(),
    )
}

/// `count` tag keys outside the same set, each offending in ONE position.
fn illegal_keys(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| Value::String(format!("bad!{index}")))
            .collect(),
    )
}

#[tokio::test]
async fn a_tag_write_refusal_fits_the_modelled_message_length() {
    let router = emulator();
    let (_, arn) = create_plan_with_tags(&router, "message-bound-target", Value::Null).await;

    // One row per offending-position count, from one position through the count at which
    // the report first passes the bound to the 400 positions the modelled `TagList`
    // maximum of 200 tags offers. `TagResource` and `CreateRescoreExecutionPlan` take the
    // same tag list, so both are driven with every row.
    for tags in [1, 2, 7, 14, 50, 200] {
        let positions = tags * 2;
        for operation in ["TagResource", "CreateRescoreExecutionPlan"] {
            let case = format!("{operation} with {tags} tags offending in {positions} positions");
            let request = match operation {
                "TagResource" => json!({ "ResourceARN": arn, "Tags": illegal_tags(tags) }),
                _ => json!({ "Name": "message-bound-create", "Tags": illegal_tags(tags) }),
            };
            let (status, answer) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {answer}"
            );
            let message = error_message(&answer);
            assert_bounded_report(&case, &message, positions);
            // The report starts at the first offending position whatever it had to drop.
            assert!(
                message.contains("tags.1.member.key"),
                "case {case:?}: the report must name the first offending position, got \
                 {message:?}"
            );
            // A report that fits still tells the caller about every position at once.
            if positions <= 13 {
                assert_eq!(
                    clauses_listed(&message),
                    positions,
                    "case {case:?}: a report that fits must state every position, got \
                     {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_untag_refusal_fits_the_modelled_message_length() {
    let router = emulator();
    let (_, arn) = create_plan_with_tags(
        &router,
        "message-bound-untag",
        json!([{ "Key": "team", "Value": "search" }]),
    )
    .await;

    // `UntagResource`'s `TagKeyList` is `@length 0..200` too, and one key offends in one
    // position, so the rows run to the modelled maximum of 200 clauses.
    for keys in [1, 2, 15, 16, 200] {
        let case = format!("UntagResource with {keys} illegal keys");
        let (status, answer) = call(
            &router,
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": illegal_keys(keys) }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        let message = error_message(&answer);
        assert_bounded_report(&case, &message, keys);
        assert!(
            message.contains("tagKeys.1.member"),
            "case {case:?}: the report must name the first offending position, got {message:?}"
        );
    }

    // Nothing a refused removal did changed the plan's tags.
    let listed = crate::harness::list_tags(&router, &arn).await;
    assert_eq!(
        listed,
        vec![("team".to_string(), "search".to_string())],
        "a refused removal must change nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_boundary_refusal_fits_the_modelled_message_length() {
    let router = emulator();
    let (_, arn) = create_plan_with_tags(&router, "message-bound-boundary", Value::Null).await;

    // The same bound holds for a refusal the boundary validator produces before any
    // handler runs, which `service.rs` words with the same report: `TagKey` is
    // `@length 1..128` and `TagValue` `@length 0..256`
    // (`smithy_query(query="shape",shape="Tag")`), so a request whose keys and values are
    // longer offends in two modelled constraints per tag.
    let long_key = "k".repeat(200);
    let long_value = "v".repeat(400);
    for tags in [1, 2, 20, 200] {
        let case = format!("TagResource with {tags} over-long keys and values");
        let request = Value::Array(
            (0..tags)
                .map(|_| json!({ "Key": long_key, "Value": long_value }))
                .collect(),
        );
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": request }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer}"
        );
        let message = error_message(&answer);
        assert_bounded_report(&case, &message, tags * 2);
        // The report never echoes the offending value, so the over-long key and value
        // cannot be what made it long.
        for offender in [&long_key, &long_value] {
            assert!(
                !message.contains(offender.as_str()),
                "case {case:?}: the report must not echo the offending value, got {message:?}"
            );
        }
    }
}
