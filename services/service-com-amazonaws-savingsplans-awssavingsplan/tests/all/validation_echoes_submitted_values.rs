// SPDX-License-Identifier: CC0-1.0

//! What a constraint-violation clause ECHOES: the value the caller submitted.
//!
//! A savingsplans refusal is stated in the AWS envelope — `N validation error(s)
//! detected: Value '<value>' at '<member path>' failed to satisfy constraint:
//! <requirement>` — and the value between the quotes is BUILT FROM THE REQUEST: it is
//! the only place the refusal reports the offending input back, so a caller that parses
//! the clause to recover what it sent must see its own bytes. The rule asserted here,
//! over every constraint class the model declares:
//!
//! * a clause names the value AT the member it reports, rendered the way that member's
//!   value renders — a list as the bracketed, comma-joined elements the caller sent, a
//!   string as the string itself, however long;
//! * the member path is rooted at the request object (`originalRequest.<member>`), the
//!   requirement is the model's, and the answer is `400 ValidationException` — which is
//!   what [`crate::tagging_error_paths`] pins and this suite does not restate;
//! * nothing is refused on a request the model admits: each row's bound is asserted
//!   from the inside too (a list AT its maximum, a token AT its maximum, the empty
//!   list), so the rule cannot be satisfied by refusing more.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, reusing
//! the harness in [`crate::savings_plans_lifecycle`].

use serde_json::{json, Value};

use crate::savings_plans_lifecycle::{text, Emulator};

/// The most ARNs `DescribeSavingsPlans.savingsPlanArns` may carry.
///
/// Source: `smithy_query(query="shape",shape="SavingsPlanArnList")` —
/// `@length {min: 0, max: 100}`.
const MAX_SAVINGS_PLAN_ARNS: usize = 100;

/// The longest `PaginationToken` any paginated operation may carry.
///
/// Source: `smithy_query(query="shape",shape="DescribeSavingsPlansRequest")` —
/// `nextToken` targets `PaginationToken`, `@length {min: 0, max: 1024}` with
/// `@pattern ^[A-Za-z0-9/=\+]+$`.
const MAX_PAGINATION_TOKEN: usize = 1024;

/// A well-formed, nonexistent Savings Plan ARN, distinct per `index`.
///
/// Built to `SavingsPlanArn`'s own expression (the UUID's version field is `[1-5]` and
/// its variant field `[89ab]`), so a row that refuses one of these is refusing it for
/// the rule under test and not for its spelling.
fn arn(index: usize) -> String {
    format!(
        "arn:aws:savingsplans::000000000000:savingsplan/{index:08x}-1111-1111-8111-111111111111"
    )
}

/// `count` distinct well-formed ARNs.
fn arns(count: usize) -> Vec<String> {
    (0..count).map(arn).collect()
}

/// The clause a list-length refusal of `savingsPlanArns` states for `sent`.
fn too_many_arns_clause(sent: &[String]) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at 'originalRequest.savingsPlanArns' failed \
         to satisfy constraint: Member must have length less than or equal to \
         {MAX_SAVINGS_PLAN_ARNS}",
        sent.join(", ")
    )
}

/// A list over its `@length` maximum is refused with the LIST the caller sent rendered
/// in the clause — not a count of its items.
///
/// The rows differ along what the rule generalises over: how far past the maximum the
/// list is (one over, two over, half again as long) and which ARNs it holds, since the
/// clause is built from them. One row past a bound is not the rule, so the maximum
/// itself and the empty list — the other published bound — are asserted as accepted
/// below.
#[tokio::test]
async fn a_list_over_its_maximum_is_refused_with_the_list_the_caller_sent() {
    let emulator = Emulator::new();

    // (case, how many ARNs the request carries)
    let refused: [(&str, usize); 3] = [
        ("one past the maximum", MAX_SAVINGS_PLAN_ARNS + 1),
        ("two past the maximum", MAX_SAVINGS_PLAN_ARNS + 2),
        ("half again as long as the maximum", MAX_SAVINGS_PLAN_ARNS * 3 / 2),
    ];

    for (case, count) in refused {
        let sent = arns(count);
        let answer = emulator
            .call(
                "DescribeSavingsPlans",
                json!({ "savingsPlanArns": sent.clone() }),
            )
            .await;
        let expected = too_many_arns_clause(&sent);
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({count} ARNs): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code, "ValidationException",
            "case {case:?} ({count} ARNs): expected ValidationException, got {:?}",
            answer.code
        );
        let message = text(&answer.body, "message");
        assert_eq!(
            message, expected,
            "case {case:?} ({count} ARNs): expected {expected:?}, got {message:?}"
        );
        // The point of the rule, stated separately so a failure says which half broke:
        // every ARN the caller sent is recoverable from the clause, and no summary of
        // them stands in for it.
        for sent in &sent {
            assert!(
                message.contains(sent.as_str()),
                "case {case:?} ({count} ARNs): the clause must name every submitted ARN; \
                 {sent:?} is missing from {message:?}"
            );
        }
        assert!(
            !message.contains(" items"),
            "case {case:?} ({count} ARNs): expected the submitted list, not a count of it, \
             got {message:?}"
        );
    }
}

/// The published bounds of the same member from the INSIDE: a list at its maximum, a
/// list well under it and the empty list are all admitted, and each answers with the
/// plans those ARNs name — none, since the ARNs are well-formed and nonexistent.
#[tokio::test]
async fn a_list_within_its_maximum_is_admitted() {
    let emulator = Emulator::new();

    // (case, how many ARNs the request carries) — the minimum (0), a short list and the
    // maximum.
    let admitted: [(&str, usize); 4] = [
        ("the empty list, the published minimum", 0),
        ("two ARNs", 2),
        ("one short of the maximum", MAX_SAVINGS_PLAN_ARNS - 1),
        ("exactly the maximum", MAX_SAVINGS_PLAN_ARNS),
    ];

    for (case, count) in admitted {
        let answer = emulator
            .call("DescribeSavingsPlans", json!({ "savingsPlanArns": arns(count) }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({count} ARNs): expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.list("savingsPlans"),
            &[] as &[Value],
            "case {case:?} ({count} ARNs): these ARNs name no plan of ours, so the answer \
             must be an empty list, got {:?}",
            answer.body
        );
    }
}

/// The same rule on a STRING member: a value over its `@length` maximum is echoed in
/// full, rather than as the bounded prefix a renderer would summarise it with.
///
/// The rows differ in how far past the maximum the value is — including far enough that
/// any fixed-prefix rendering would have to cut it — and the member is on a different
/// operation from the list rows above, so the rule is the validator's and not one
/// member's.
#[tokio::test]
async fn a_string_over_its_maximum_is_refused_with_the_whole_value_the_caller_sent() {
    let emulator = Emulator::new();

    // (case, how long the token is)
    let refused: [(&str, usize); 3] = [
        ("one byte past the maximum", MAX_PAGINATION_TOKEN + 1),
        ("twice the maximum", MAX_PAGINATION_TOKEN * 2),
        ("eight times the maximum", MAX_PAGINATION_TOKEN * 8),
    ];

    for (case, length) in refused {
        // Every byte satisfies `PaginationToken`'s pattern, so the only rule the value
        // breaks is the length bound the row is about.
        let token = "A".repeat(length);
        for operation in [
            "DescribeSavingsPlans",
            "DescribeSavingsPlanRates",
            "DescribeSavingsPlansOfferings",
        ] {
            let mut request = json!({ "nextToken": token.clone() });
            if operation == "DescribeSavingsPlanRates" {
                request["savingsPlanId"] = json!("00000000-0000-1000-8000-000000000000");
            }
            let answer = emulator.call(operation, request).await;
            let expected = format!(
                "1 validation error detected: Value '{token}' at 'originalRequest.nextToken' \
                 failed to satisfy constraint: Member must have length less than or equal to \
                 {MAX_PAGINATION_TOKEN}"
            );
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400, got {} {:?}",
                answer.status, answer.code
            );
            let message = text(&answer.body, "message");
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the whole submitted token echoed, got \
                 {message:?}"
            );
        }
    }
}

/// A token AT its maximum length is admitted — it is simply not a token this service
/// issued, which is the operation's own refusal and not a boundary one.
#[tokio::test]
async fn a_token_at_its_maximum_length_passes_the_boundary() {
    let emulator = Emulator::new();
    let token = "A".repeat(MAX_PAGINATION_TOKEN);
    let answer = emulator
        .call("DescribeSavingsPlans", json!({ "nextToken": token.clone() }))
        .await;
    let message = text(&answer.body, "message");
    assert_eq!(
        answer.status, 400,
        "a token at the maximum: expected the operation's own 400, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        !message.contains("validation error detected"),
        "a token at the maximum: expected the operation's own refusal rather than a boundary \
         clause, got {message:?}"
    );
}

/// A constraint broken by one ELEMENT of a list is still reported at that element, with
/// the element's own value: the rule is "the value AT the member the clause names", and
/// for an element violation that member is the element.
///
/// The rows differ in which position of the list is at fault and how long the list is,
/// so the clause's path is read off the walk rather than off one example.
#[tokio::test]
async fn an_element_violation_echoes_the_element_rather_than_the_list() {
    let emulator = Emulator::new();

    // (case, the 1-based position the bad ARN is sent at, how long the list is)
    let rows: [(&str, usize, usize); 3] = [
        ("the only element", 1, 1),
        ("the first of several", 1, 4),
        ("the last of several", 3, 3),
    ];

    for (case, position, length) in rows {
        let bad = "not-an-arn";
        let sent: Vec<String> = (1..=length)
            .map(|index| {
                if index == position {
                    bad.to_string()
                } else {
                    arn(index)
                }
            })
            .collect();
        let answer = emulator
            .call("DescribeSavingsPlans", json!({ "savingsPlanArns": sent }))
            .await;
        let message = text(&answer.body, "message");
        let expected_path = format!("'originalRequest.savingsPlanArns.{position}.member'");
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert!(
            message.starts_with(&format!("1 validation error detected: Value '{bad}' at ")),
            "case {case:?}: expected the element's own value echoed, got {message:?}"
        );
        assert!(
            message.contains(&expected_path),
            "case {case:?}: expected the clause at {expected_path}, got {message:?}"
        );
    }
}
