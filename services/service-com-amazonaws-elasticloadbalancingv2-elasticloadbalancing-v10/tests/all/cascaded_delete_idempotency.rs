// SPDX-License-Identifier: CC0-1.0

//! Deleting a listener or a rule a CASCADE already released, driven over the wire exactly
//! as a client tearing a deployment down drives it.
//!
//! One rule is asserted here, over a SET of teardown orders rather than over the one call
//! a recording happens to carry: a listener's RULES are released with the listener (the
//! ones a caller added and the default rule it minted with itself), and with the load
//! balancer that contained the listener — "deleting a load balancer also deletes its
//! listeners" — so a `DeleteRule` naming a rule that cascade already removed asks for a
//! deletion that has been performed and COMPLETES, while a rule the caller deleted
//! itself, and an identifier this store never held, are the modelled `RuleNotFound`.
//!
//! The LISTENER is not covered by that: `DeleteListener` models
//! `ListenerNotFoundException` and answers it for a listener a cascade removed as well as
//! for one the caller deleted, which the rows below pin too.
//!
//! The rows differ along what the rule generalises over: which container was deleted (the
//! load balancer or the listener), which resource the delete then names (a rule a caller
//! added, a listener's default rule, the listener itself), how many times the delete is
//! repeated, and the two ways an identifier comes to name nothing without a cascade (the
//! caller deleted it, or nothing ever carried it).
//!
//! And one message is pinned: `SetRulePriorities` addresses a LIST of priorities, so its
//! `RuleNotFound` is the collection-level wording, naming no value.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, forward_to, host_header,
};
use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, first_text, members, p, Reply, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `ListenerNotFoundException`
/// (`smithy_query(query="operation",operation="DeleteListener")`: `awsQueryError` code
/// `ListenerNotFound`).
const LISTENER_NOT_FOUND: &str = "ListenerNotFound";

/// The awsQuery error code of `RuleNotFoundException`, which `DeleteRule` and
/// `SetRulePriorities` both model.
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// Send one `DeleteRule` request.
async fn delete_rule(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DeleteRule");
    form.push(p("RuleArn", arn));
    call(router, &form).await
}

/// Send one `DeleteListener` request.
async fn delete_listener(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", arn));
    call(router, &form).await
}

/// Send one `DeleteLoadBalancer` request.
async fn delete_load_balancer(router: &MultiServiceRouter, case: &str, arn: &str) {
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeleteLoadBalancer, got {} body {}",
        reply.status, reply.body
    );
}

/// The ARN of the rule a listener mints with itself.
async fn default_rule_arn(router: &MultiServiceRouter, case: &str, listener_arn: &str) -> String {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "Rules")
        .into_iter()
        .find(|rule| first_text(rule, "IsDefault").as_deref() == Some("true"))
        .and_then(|rule| first_text(&rule, "RuleArn"))
        .unwrap_or_else(|| panic!("case {case:?}: every listener reports one, got {}", reply.body))
}

/// Assert that deleting the rule `arn` names COMPLETES, twice over, so the idempotency
/// is not a one-off.
async fn assert_rule_delete_completes(router: &MultiServiceRouter, case: &str, arn: &str) {
    for attempt in 1..=2 {
        let reply = delete_rule(router, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} attempt {attempt}: the deletion has already been performed, so the \
             call completes, got {} body {}",
            reply.status, reply.body
        );
    }
}

/// Assert that deleting the listener `arn` names is REFUSED with the modelled
/// `ListenerNotFound`, twice over.
async fn assert_listener_delete_refused(router: &MultiServiceRouter, case: &str, arn: &str) {
    for attempt in 1..=2 {
        let reply = delete_listener(router, arn).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, LISTENER_NOT_FOUND),
            "case {case:?} attempt {attempt}: expected 400 {LISTENER_NOT_FOUND}, got {} body {}",
            reply.status,
            reply.body
        );
    }
}

/// What the fixture holds: a load balancer, a listener of it, a rule a caller added and
/// that listener's default rule.
struct Fixture {
    load_balancer: String,
    listener: String,
    rule: String,
    default_rule: String,
}

/// Build the fixture on a fresh emulator.
async fn fixture(router: &MultiServiceRouter, case: &str, name: &str) -> Fixture {
    let load_balancer = create_load_balancer(router, name).await;
    let group = create_http_target_group(router, &format!("{name}-tg")).await;
    let listener = create_listener(router, &load_balancer, &group, 80).await;
    let rule = add_rule(
        router,
        case,
        &listener,
        10,
        &[host_header("cascade.example.com"), forward_to(&group)].concat(),
    )
    .await;
    let default_rule = default_rule_arn(router, case, &listener).await;
    Fixture {
        load_balancer,
        listener,
        rule,
        default_rule,
    }
}

/// A rule released WITH its listener — directly, or with the load balancer that contained
/// the listener — is deleted idempotently, while the LISTENER such a cascade removed is
/// the modelled refusal.
///
/// The rows are the teardown orders a client walks: delete the listener and then its
/// rules, and delete the load balancer and then its listener and its rules.
#[tokio::test]
async fn a_rule_released_with_its_container_is_deleted_idempotently() {
    // Row 1: the listener goes, then the rules it carried.
    {
        let case = "the listener deleted before its rules";
        let (router, _engine) = emulator();
        let held = fixture(&router, case, "cascade-listener-alb").await;
        let deleted = delete_listener(&router, &held.listener).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 deleting the listener, got {} body {}",
            deleted.status, deleted.body
        );
        for (what, arn) in [
            ("the rule a caller added", &held.rule),
            ("the listener's default rule", &held.default_rule),
        ] {
            assert_rule_delete_completes(&router, &format!("{case}: {what}"), arn).await;
        }
    }

    // Row 2: the load balancer goes, then its listener and that listener's rules.
    {
        let case = "the load balancer deleted before its listener";
        let (router, _engine) = emulator();
        let held = fixture(&router, case, "cascade-lb-alb").await;
        delete_load_balancer(&router, case, &held.load_balancer).await;
        assert_listener_delete_refused(
            &router,
            &format!("{case}: the listener"),
            &held.listener,
        )
        .await;
        for (what, arn) in [
            ("the rule a caller added", &held.rule),
            ("the listener's default rule", &held.default_rule),
        ] {
            assert_rule_delete_completes(&router, &format!("{case}: {what}"), arn).await;
        }
    }
}

/// The control: an identifier that names nothing WITHOUT a cascade is the modelled
/// refusal — a rule or a listener the caller deleted itself, and one nothing ever carried.
#[tokio::test]
async fn a_delete_the_caller_already_made_and_an_identifier_nothing_carried_are_refused() {
    let (router, _engine) = emulator();
    let held = fixture(&router, "the control fixture", "cascade-control-alb").await;

    // The caller's own delete of the rule: the second one is refused, because no cascade
    // performed it.
    let first = delete_rule(&router, &held.rule).await;
    assert_eq!(
        first.status, 200,
        "case the caller's own rule delete: expected 200, got {} body {}",
        first.status, first.body
    );
    let again = delete_rule(&router, &held.rule).await;
    assert_eq!(
        (again.status, again.error_code().as_str()),
        (400, RULE_NOT_FOUND),
        "case deleting the caller's own deleted rule again: expected 400 {RULE_NOT_FOUND}, got \
         {} body {}",
        again.status,
        again.body
    );

    // And of the listener, with its load balancer still there.
    let first = delete_listener(&router, &held.listener).await;
    assert_eq!(
        first.status, 200,
        "case the caller's own listener delete: expected 200, got {} body {}",
        first.status, first.body
    );
    let again = delete_listener(&router, &held.listener).await;
    assert_eq!(
        (again.status, again.error_code().as_str()),
        (400, LISTENER_NOT_FOUND),
        "case deleting the caller's own deleted listener again: expected 400 \
         {LISTENER_NOT_FOUND}, got {} body {}",
        again.status,
        again.body
    );

    // Identifiers nothing ever carried: well formed, of this account and Region, naming a
    // load balancer, a listener and a rule this store never held.
    let unknown_listener = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:listener/app/nevermade/0123456789abcdef/fedcba9876543210",
        account()
    );
    let unknown_rule = format!("{unknown_listener}/0123456789abcdef")
        .replacen(":listener/", ":listener-rule/", 1);
    let refused = delete_listener(&router, &unknown_listener).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, LISTENER_NOT_FOUND),
        "case a listener ARN nothing ever carried: expected 400 {LISTENER_NOT_FOUND}, got {} \
         body {}",
        refused.status,
        refused.body
    );
    let refused = delete_rule(&router, &unknown_rule).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, RULE_NOT_FOUND),
        "case a rule ARN nothing ever carried: expected 400 {RULE_NOT_FOUND}, got {} body {}",
        refused.status,
        refused.body
    );
}

/// `SetRulePriorities` addresses a LIST of priorities, so the refusal for a rule it cannot
/// resolve is about that selection and names no value — the collection-level wording,
/// exactly as every other list-addressing refusal of this service is worded.
///
/// The rows are the ways an entry fails to resolve: a rule the caller deleted, a rule
/// released with its listener, a well-formed ARN nothing ever carried, and a value that is
/// no rule ARN at all — each alone and beside an entry that does resolve.
#[tokio::test]
async fn the_priorities_refusal_names_no_value() {
    let (router, _engine) = emulator();
    let held = fixture(&router, "the priorities fixture", "priorities-alb").await;
    let group = create_http_target_group(&router, "priorities-live-tg").await;
    let live_listener = create_listener(&router, &held.load_balancer, &group, 8080).await;
    let live_rule = add_rule(
        &router,
        "a rule that resolves",
        &live_listener,
        10,
        &[host_header("live.example.com"), forward_to(&group)].concat(),
    )
    .await;

    // A rule the caller deleted, and one released with its listener.
    let deleted = delete_rule(&router, &held.rule).await;
    assert_eq!(
        deleted.status, 200,
        "case delete the rule first: expected 200, got {} body {}",
        deleted.status, deleted.body
    );
    let cascaded = add_rule(
        &router,
        "a rule to cascade away",
        &held.listener,
        20,
        &[host_header("cascaded.example.com"), forward_to(&group)].concat(),
    )
    .await;
    let listener_gone = delete_listener(&router, &held.listener).await;
    assert_eq!(
        listener_gone.status, 200,
        "case delete the listener: expected 200, got {} body {}",
        listener_gone.status, listener_gone.body
    );

    let unknown = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:listener-rule/app/nevermade/0123456789abcdef/fedcba9876543210/0123456789abcdef",
        account()
    );
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "a rule the caller deleted",
            vec![(held.rule.as_str(), "40")],
        ),
        (
            "a rule released with its listener",
            vec![(cascaded.as_str(), "40")],
        ),
        (
            "a rule ARN nothing ever carried",
            vec![(unknown.as_str(), "40")],
        ),
        ("a value that is no rule ARN", vec![("not-a-rule-arn", "40")]),
        (
            "an entry that resolves and one that does not",
            vec![(live_rule.as_str(), "41"), (unknown.as_str(), "40")],
        ),
    ];

    for (case, pairs) in cases {
        let mut form = action("SetRulePriorities");
        for (index, (rule_arn, priority)) in pairs.iter().enumerate() {
            let member = index + 1;
            form.push(p(&format!("RulePriorities.member.{member}.RuleArn"), rule_arn));
            form.push(p(
                &format!("RulePriorities.member.{member}.Priority"),
                priority,
            ));
        }
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, RULE_NOT_FOUND),
            "case {case:?}: expected 400 {RULE_NOT_FOUND}, got {} body {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.text("Message").unwrap_or_default(),
            "One or more priorities not found",
            "case {case:?}: the refusal is about the selection and names no value, got body {}",
            reply.body
        );
    }
}
