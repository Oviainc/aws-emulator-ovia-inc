// SPDX-License-Identifier: CC0-1.0

//! Re-prioritising a listener's RULES, driven over the wire exactly as a client drives
//! it: rules added with `CreateRule`, moved with `SetRulePriorities` and read back with
//! `DescribeRules`.
//!
//! One rule is asserted here, over a SET of reorderings rather than over the one call a
//! recording happens to carry: `SetRulePriorities` moves each rule the request names to
//! the priority it names, leaves every rule it does not name at the priority it already
//! had, and refuses only a result in which two rules OF ONE LISTENER evaluate at the same
//! priority — the priority namespace is the listener's, so the same number already in use
//! on another listener is no conflict.
//!
//! The rows differ along what that rule generalises over: how many rules one call moves
//! (one, two, three, every rule of the listener), the kind of reordering (to a free
//! priority, a swap, a three-way cycle, a move onto a number another listener uses, a
//! move onto the number the rule already has), and the priority moved to (including both
//! published bounds of `RulePriority`, range 1..50000, and one past each of them).
//!
//! Every accepted move is read back through `DescribeRules` rather than trusted from the
//! write's own answer, and after every refusal both listeners' rules are read again to
//! prove no priority moved.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, forward_to, host_header, Form,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `RuleNotFoundException`, which `SetRulePriorities` models
/// (`smithy_query(query="operation",operation="SetRulePriorities")`: `awsQueryError` code
/// `RuleNotFound`).
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery error code of `PriorityInUseException`, which `SetRulePriorities` models.
const PRIORITY_IN_USE: &str = "PriorityInUse";

/// The awsQuery error code of `OperationNotPermittedException`, which
/// `SetRulePriorities` models.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The priorities the rules of the listener under test start at.
const STARTING_PRIORITIES: [i32; 3] = [10, 20, 30];

/// The priorities the rules of the OTHER listener of the same load balancer start at —
/// numbers the listener under test does not use, so a move onto one of them is a move
/// onto a priority that is in use elsewhere and free here.
const OTHER_PRIORITIES: [i32; 2] = [100, 200];

/// Two listeners of one load balancer, each carrying rules a caller added.
struct Fixture {
    /// The listener whose rules the call under test moves.
    listener: String,
    /// Another listener of the same load balancer, which no call under test names.
    other: String,
    /// The rules of [`Self::listener`], in the order they were created — so at
    /// [`STARTING_PRIORITIES`].
    rules: Vec<String>,
}

/// Build the fixture on a fresh emulator: a load balancer with two HTTP listeners, three
/// rules on one and two on the other.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let lb = create_load_balancer(router, "rule-priority-alb").await;
    let group = create_http_target_group(router, "rule-priority-tg").await;
    let listener = create_listener(router, &lb, &group, 80).await;
    let other = create_listener(router, &lb, &group, 8080).await;
    let mut rules = Vec::new();
    for priority in STARTING_PRIORITIES {
        rules.push(
            add_rule(
                router,
                &format!("seed the rule at priority {priority}"),
                &listener,
                priority,
                &[
                    host_header(&format!("p{priority}.example.com")),
                    forward_to(&group),
                ]
                .concat(),
            )
            .await,
        );
    }
    for priority in OTHER_PRIORITIES {
        add_rule(
            router,
            &format!("seed the other listener's rule at priority {priority}"),
            &other,
            priority,
            &[
                host_header(&format!("o{priority}.example.com")),
                forward_to(&group),
            ]
            .concat(),
        )
        .await;
    }
    Fixture {
        listener,
        other,
        rules,
    }
}

/// The form fields of one `SetRulePriorities` request: one `RulePriorityPair` per entry,
/// in the order given. An EMPTY string leaves that member out of the pair altogether,
/// which is how the rows below ask for a pair naming no rule or no priority.
fn priority_pairs(pairs: &[(&str, &str)]) -> Form {
    let mut form = action("SetRulePriorities");
    for (index, (rule_arn, priority)) in pairs.iter().enumerate() {
        let member = index + 1;
        if !rule_arn.is_empty() {
            form.push(p(
                &format!("RulePriorities.member.{member}.RuleArn"),
                rule_arn,
            ));
        }
        if !priority.is_empty() {
            form.push(p(
                &format!("RulePriorities.member.{member}.Priority"),
                priority,
            ));
        }
    }
    form
}

/// Send one `SetRulePriorities` request.
async fn set_priorities(router: &MultiServiceRouter, pairs: &[(&str, &str)]) -> Reply {
    call(router, &priority_pairs(pairs)).await
}

/// `(RuleArn, Priority)` of every rule in one reply, in the order it reported them.
fn reported(reply: &Reply) -> Vec<(String, String)> {
    members(&reply.body, "Rules")
        .iter()
        .map(|rule| {
            (
                first_text(rule, "RuleArn").unwrap_or_default(),
                first_text(rule, "Priority").unwrap_or_default(),
            )
        })
        .collect()
}

/// Read one listener's rules and answer `(RuleArn, Priority)` for each, in the order the
/// read reported them — which is evaluation order, the listener's default rule last.
async fn stored_priorities(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
) -> Vec<(String, String)> {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200 from DescribeRules, got {} {}",
        reply.status, reply.body
    );
    reported(&reply)
}

/// The ARN of a listener's DEFAULT rule: the one rule a read reports with the priority
/// `default`.
async fn default_rule_arn(router: &MultiServiceRouter, listener_arn: &str) -> String {
    stored_priorities(router, "the default rule", listener_arn)
        .await
        .into_iter()
        .find(|(_, priority)| priority == "default")
        .map(|(arn, _)| arn)
        .expect("case the default rule: a listener always reports one")
}

/// How many rules one `SetRulePriorities` call may name before the emulator refuses the
/// list: the bound the handler puts on `RulePriorities`, which the model leaves
/// unconstrained (the same bound its sibling read puts on `DescribeRules`'s `RuleArns`).
const MAX_RULES_PER_CALL: usize = 100;

/// One row of the refusal table: the case, the pairs the request carries, and the
/// awsQuery error code the refusal must answer under (empty when the code is the
/// boundary validator's rather than this service's).
type Refusal<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str);

/// One row of the reordering table: the case, which of the listener's rules the call
/// moves and to which priority, and the priority every rule of that listener then
/// evaluates at (by its position in [`Fixture::rules`]).
struct Row {
    case: &'static str,
    moves: Vec<(usize, i32)>,
    expect: [i32; 3],
}

/// What `SetRulePriorities` moves, and what it leaves alone.
#[tokio::test]
async fn rules_move_to_the_priorities_the_request_names_and_others_keep_theirs() {
    let rows = vec![
        Row {
            case: "one rule to a priority nothing holds",
            moves: vec![(0, 40)],
            expect: [40, 20, 30],
        },
        Row {
            case: "two rules swapping priorities in one call",
            moves: vec![(0, 20), (1, 10)],
            expect: [20, 10, 30],
        },
        Row {
            case: "three rules cycling their priorities",
            moves: vec![(0, 20), (1, 30), (2, 10)],
            expect: [20, 30, 10],
        },
        Row {
            case: "a priority in use on another listener of the same load balancer",
            moves: vec![(0, OTHER_PRIORITIES[0])],
            expect: [OTHER_PRIORITIES[0], 20, 30],
        },
        Row {
            case: "the lowest priority the model publishes",
            moves: vec![(1, 1)],
            expect: [10, 1, 30],
        },
        Row {
            case: "the highest priority the model publishes",
            moves: vec![(1, 50_000)],
            expect: [10, 50_000, 30],
        },
        Row {
            case: "a rule set to the priority it already has",
            moves: vec![(1, 20)],
            expect: [10, 20, 30],
        },
        Row {
            case: "every rule of the listener reversed in one call",
            moves: vec![(0, 3), (1, 2), (2, 1)],
            expect: [3, 2, 1],
        },
        Row {
            case: "a rule onto a priority a later pair in the same call frees",
            moves: vec![(0, 20), (1, 40)],
            expect: [20, 40, 30],
        },
    ];

    for row in rows {
        let case = row.case;
        // A fresh emulator per row: each reordering starts from the same rules at
        // `STARTING_PRIORITIES`, so a row cannot depend on the row before it.
        let (router, _engine) = emulator();
        let fixture = fixture(&router).await;
        let other_before = stored_priorities(&router, case, &fixture.other).await;

        let pairs: Vec<(String, String)> = row
            .moves
            .iter()
            .map(|(index, priority)| (fixture.rules[*index].clone(), priority.to_string()))
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(arn, priority)| (arn.as_str(), priority.as_str()))
            .collect();
        let reply = set_priorities(&router, &borrowed).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );

        // The answer reports the rules the request named, each at its new priority, in
        // the order it named them — and nothing else.
        let answered = reported(&reply);
        assert_eq!(
            answered, pairs,
            "case {case}: expected the moved rules {pairs:?}, got {answered:?}"
        );
        for rule in members(&reply.body, "Rules") {
            let is_default = first_text(&rule, "IsDefault").unwrap_or_default();
            assert_eq!(
                is_default, "false",
                "case {case}: expected a moved rule to be no default rule, got {is_default:?}"
            );
        }

        // Read back: every rule of the listener at the priority this row expects, in
        // evaluation order (lowest value first) with the default rule last.
        let mut want: Vec<(String, String)> = fixture
            .rules
            .iter()
            .zip(row.expect.iter())
            .map(|(arn, priority)| (arn.clone(), priority.to_string()))
            .collect();
        want.sort_by_key(|(_, priority)| priority.parse::<i32>().unwrap_or_default());
        want.push((
            default_rule_arn(&router, &fixture.listener).await,
            "default".to_string(),
        ));
        let stored = stored_priorities(&router, case, &fixture.listener).await;
        assert_eq!(
            stored, want,
            "case {case}: expected the listener's rules at {want:?}, got {stored:?}"
        );

        // A listener the request did not name keeps every priority it had.
        let other_after = stored_priorities(&router, case, &fixture.other).await;
        assert_eq!(
            other_after, other_before,
            "case {case}: expected another listener's rules unchanged at {other_before:?}, \
             got {other_after:?}"
        );
    }
}

/// What `SetRulePriorities` refuses, and that a refused call moves no priority at all —
/// so the priorities a later read reports are exactly the ones it accepted.
#[tokio::test]
async fn a_refused_reordering_moves_nothing() {
    let (router, _engine) = emulator();
    let fixture = fixture(&router).await;
    // A rule that was deleted: a well-formed rule ARN this store no longer holds.
    let gone = fixture.rules[2].clone();
    let mut form = action("DeleteRule");
    form.push(p("RuleArn", &gone));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the third rule: expected 200, got {} {}",
        deleted.status, deleted.body
    );
    let default_rule = default_rule_arn(&router, &fixture.listener).await;
    let before = stored_priorities(&router, "before any refusal", &fixture.listener).await;
    let other_before = stored_priorities(&router, "before any refusal", &fixture.other).await;
    let first = fixture.rules[0].clone();
    let second = fixture.rules[1].clone();
    // One pair past the bound on how many rules one call may name. Each names a rule the
    // store does not hold, so the list is refused for its LENGTH before any entry is
    // resolved.
    let past_bound: Vec<(String, String)> = (0..=MAX_RULES_PER_CALL)
        .map(|index| (format!("{first}{index}"), "40".to_string()))
        .collect();
    let past_bound: Vec<(&str, &str)> = past_bound
        .iter()
        .map(|(arn, priority)| (arn.as_str(), priority.as_str()))
        .collect();

    let refusals: Vec<Refusal<'_>> = vec![
        (
            "a rule that was deleted",
            vec![(gone.as_str(), "40")],
            RULE_NOT_FOUND,
        ),
        (
            "a value that is no rule ARN",
            vec![("not-a-rule-arn", "40")],
            RULE_NOT_FOUND,
        ),
        (
            "the listener's default rule",
            vec![(default_rule.as_str(), "40")],
            OPERATION_NOT_PERMITTED,
        ),
        (
            "a priority a rule the request does not name holds",
            vec![(first.as_str(), "20")],
            PRIORITY_IN_USE,
        ),
        (
            // One call cannot ask for two rules at one priority, whatever listeners they
            // belong to: the list does not say which of them evaluates there, so it is the
            // request that has to be fixed.
            "two rules of one listener onto one priority",
            vec![(first.as_str(), "40"), (second.as_str(), "40")],
            VALIDATION_ERROR,
        ),
        (
            "one rule named twice",
            vec![(second.as_str(), "40"), (second.as_str(), "41")],
            VALIDATION_ERROR,
        ),
        ("a pair naming no rule", vec![("", "40")], VALIDATION_ERROR),
        (
            "a pair naming no priority",
            vec![(second.as_str(), "")],
            VALIDATION_ERROR,
        ),
        (
            "one rule found and a later one that is not",
            vec![(second.as_str(), "40"), ("not-a-rule-arn", "41")],
            RULE_NOT_FOUND,
        ),
        (
            "a priority below the published range",
            vec![(second.as_str(), "0")],
            "",
        ),
        (
            "a priority above the published range",
            vec![(second.as_str(), "50001")],
            "",
        ),
        (
            "one rule past the bound on how many a call may name",
            past_bound,
            VALIDATION_ERROR,
        ),
    ];

    for (case, pairs, code) in refusals {
        let reply = set_priorities(&router, &pairs).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400, got {} {}",
            reply.status, reply.body
        );
        if !code.is_empty() {
            assert_eq!(
                reply.error_code().as_str(),
                code,
                "case {case}: expected {code}, got {}",
                reply.body
            );
        }
        let stored = stored_priorities(&router, case, &fixture.listener).await;
        assert_eq!(
            stored, before,
            "case {case}: expected a refused call to move nothing, {before:?}, got {stored:?}"
        );
        let other_after = stored_priorities(&router, case, &fixture.other).await;
        assert_eq!(
            other_after, other_before,
            "case {case}: expected another listener's rules unchanged at {other_before:?}, \
             got {other_after:?}"
        );
    }
}
