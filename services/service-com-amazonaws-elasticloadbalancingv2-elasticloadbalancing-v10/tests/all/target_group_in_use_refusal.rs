// SPDX-License-Identifier: CC0-1.0

//! The refusal `DeleteTargetGroup` answers a still-referenced target group with, driven
//! over the wire exactly as a client drives it.
//!
//! One rule is asserted here, over a SET of inputs rather than the one call a recording
//! happens to carry: this refusal's message is a function of the REQUEST alone — the
//! constant text plus the identifier the caller sent — and never carries the
//! server-generated identifier of whichever resource holds the reference, so the same
//! request is refused with the same text whatever is holding the group.
//!
//! The rows differ along what that rule generalises over: which kind of action holds the
//! reference (a listener's default action or a rule's), the form the action reaches the
//! group through (`TargetGroupArn` directly or a `ForwardConfig` tuple, alone or weighted
//! beside another group), how many holders there are (one listener, two listeners, a
//! listener and a rule), and the protocol and port of the group itself.
//!
//! After every refusal the group is read back through `DescribeTargetGroups` to prove the
//! refused delete removed nothing.

use crate::provisioning_loop::{
    account, action, call, emulator, first_text, p, Reply, REGION,
};
use crate::target_group_attributes_and_modification::{create_target_group, unescaped};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `ResourceInUseException`, the ONE error
/// `DeleteTargetGroup` models
/// (`smithy_query(query="operation",operation="DeleteTargetGroup")`: errors
/// `[ResourceInUseException]`, `awsQueryError` code `ResourceInUse`, httpError 400).
const RESOURCE_IN_USE: &str = "ResourceInUse";

/// The awsQuery error code of `TargetGroupNotFoundException`, which
/// `DescribeTargetGroups` models.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// The refusal text for the target group `arn` names: the constant prefix, the ARN the
/// REQUEST carried, and the invariant phrase — nothing read out of the store.
fn in_use_message(arn: &str) -> String {
    format!("Target group '{arn}' is currently in use by a listener or a rule")
}

/// How the reference that holds a target group is wired.
///
/// The forms an action can reach a target group through are the members of `Action`
/// (`smithy_query(query="shape",shape="Action,ForwardActionConfig,TargetGroupTuple")`: a
/// `forward` action names `TargetGroupArn` directly or carries a `ForwardConfig` of
/// `TargetGroupTuple`s), and an action is either one of a listener's default actions or
/// one of a rule's.
#[derive(Debug, Clone, Copy)]
enum Holder {
    /// One listener's default action names the group directly.
    DefaultAction,
    /// One listener's default action reaches it through a `ForwardConfig` tuple.
    DefaultForwardConfig,
    /// A rule added to a listener names it directly, the listener forwarding elsewhere.
    RuleAction,
    /// A rule reaches it through a weighted `ForwardConfig` beside another group.
    WeightedRuleForwardConfig,
    /// Two listeners of the same load balancer both forward to it.
    TwoListeners,
    /// A listener's default action AND a rule of another listener hold it at once.
    ListenerAndRule,
}

/// One row: the case, the `CreateTargetGroup` fields of the group whose delete is
/// refused, and what holds it.
struct Row {
    case: &'static str,
    group: &'static [(&'static str, &'static str)],
    holder: Holder,
}

/// The VPC every target group below lives in.
const VPC: &str = "vpc-12345678";

/// The group an action that needs a second target group forwards to.
const FILLER: &[(&str, &str)] = &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", VPC)];

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a default action naming the group directly",
            group: &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", VPC)],
            holder: Holder::DefaultAction,
        },
        Row {
            case: "a default action reaching an HTTPS group through a forward config",
            group: &[("Protocol", "HTTPS"), ("Port", "443"), ("VpcId", VPC)],
            holder: Holder::DefaultForwardConfig,
        },
        Row {
            case: "a rule naming the group directly",
            group: &[("Protocol", "HTTP"), ("Port", "8080"), ("VpcId", VPC)],
            holder: Holder::RuleAction,
        },
        Row {
            case: "a weighted forward config on a rule, beside another group",
            group: &[
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("VpcId", VPC),
                ("TargetType", "ip"),
            ],
            holder: Holder::WeightedRuleForwardConfig,
        },
        Row {
            case: "two listeners of one load balancer forwarding to the same group",
            group: &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", VPC)],
            holder: Holder::TwoListeners,
        },
        Row {
            case: "a default action and another listener's rule holding it at once",
            group: &[("Protocol", "HTTP"), ("Port", "3000"), ("VpcId", VPC)],
            holder: Holder::ListenerAndRule,
        },
    ]
}

/// Create a load balancer across two subnets and answer its ARN.
async fn create_load_balancer(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: creating the load balancer expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case {case}: no LoadBalancerArn in {}", reply.body))
}

/// Create an HTTP listener on `port` carrying `actions` as its default actions, and
/// answer its ARN.
async fn create_listener(
    router: &MultiServiceRouter,
    case: &str,
    lb_arn: &str,
    port: i32,
    actions: &[(String, String)],
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &port.to_string()));
    form.extend(actions.iter().cloned());
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: creating the listener on port {port} expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case {case}: no ListenerArn in {}", reply.body))
}

/// Add one rule carrying `actions` to `listener_arn` and answer its ARN.
async fn create_rule(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
    priority: i32,
    actions: &[(String, String)],
) -> String {
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("Priority", &priority.to_string()));
    form.push(p("Conditions.member.1.Field", "host-header"));
    form.push(p("Conditions.member.1.Values.member.1", "rule.example.com"));
    form.extend(actions.iter().cloned());
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: creating the rule expected 200, got {} body {}",
        reply.status, reply.body
    );
    first_text(&reply.body, "RuleArn")
        .unwrap_or_else(|| panic!("case {case}: no RuleArn in {}", reply.body))
}

/// A default action naming `tg_arn` directly.
fn default_direct(tg_arn: &str) -> Form {
    vec![
        p("DefaultActions.member.1.Type", "forward"),
        p("DefaultActions.member.1.TargetGroupArn", tg_arn),
    ]
}

/// A default action reaching `tg_arn` through a `ForwardConfig` tuple.
fn default_forward_config(tg_arn: &str) -> Form {
    vec![
        p("DefaultActions.member.1.Type", "forward"),
        p(
            "DefaultActions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
            tg_arn,
        ),
    ]
}

/// A rule action naming `tg_arn` directly.
fn rule_direct(tg_arn: &str) -> Form {
    vec![
        p("Actions.member.1.Type", "forward"),
        p("Actions.member.1.TargetGroupArn", tg_arn),
    ]
}

/// A rule action splitting traffic between `tg_arn` and `other`, by weight.
fn rule_weighted(tg_arn: &str, other: &str) -> Form {
    vec![
        p("Actions.member.1.Type", "forward"),
        p(
            "Actions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
            tg_arn,
        ),
        p("Actions.member.1.ForwardConfig.TargetGroups.member.1.Weight", "30"),
        p(
            "Actions.member.1.ForwardConfig.TargetGroups.member.2.TargetGroupArn",
            other,
        ),
        p("Actions.member.1.ForwardConfig.TargetGroups.member.2.Weight", "70"),
    ]
}

/// Wire `holder` up so it references `group`, and answer every server-generated
/// identifier the wiring minted — the ids AWS's refusal never discloses.
async fn hold(
    router: &MultiServiceRouter,
    case: &str,
    lb_arn: &str,
    group: &str,
    other: &str,
    holder: Holder,
) -> Vec<String> {
    match holder {
        Holder::DefaultAction => {
            vec![create_listener(router, case, lb_arn, 80, &default_direct(group)).await]
        }
        Holder::DefaultForwardConfig => {
            vec![create_listener(router, case, lb_arn, 80, &default_forward_config(group)).await]
        }
        Holder::RuleAction => {
            let listener = create_listener(router, case, lb_arn, 80, &default_direct(other)).await;
            let rule = create_rule(router, case, &listener, 10, &rule_direct(group)).await;
            vec![listener, rule]
        }
        Holder::WeightedRuleForwardConfig => {
            let listener = create_listener(router, case, lb_arn, 80, &default_direct(other)).await;
            let rule =
                create_rule(router, case, &listener, 49_999, &rule_weighted(group, other)).await;
            vec![listener, rule]
        }
        Holder::TwoListeners => {
            let first = create_listener(router, case, lb_arn, 80, &default_direct(group)).await;
            let second =
                create_listener(router, case, lb_arn, 8080, &default_forward_config(group)).await;
            vec![first, second]
        }
        Holder::ListenerAndRule => {
            let first = create_listener(router, case, lb_arn, 80, &default_direct(group)).await;
            let second = create_listener(router, case, lb_arn, 8080, &default_direct(other)).await;
            let rule = create_rule(router, case, &second, 1, &rule_direct(group)).await;
            vec![first, second, rule]
        }
    }
}

/// What `DescribeTargetGroups` reports for one target group.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", arn));
    call(router, &form).await
}

#[tokio::test]
async fn the_in_use_refusal_is_built_from_the_request_and_names_no_holder() {
    for row in rows() {
        let case = row.case;
        let (router, _engine) = emulator();
        let lb = create_load_balancer(&router, case, "inuse-alb").await;
        let group = create_target_group(&router, "inuse-tg", row.group).await;
        let other = create_target_group(&router, "inuse-other-tg", FILLER).await;
        let minted = hold(&router, case, &lb, &group, &other, row.holder).await;

        let mut form = action("DeleteTargetGroup");
        form.push(p("TargetGroupArn", &group));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, RESOURCE_IN_USE),
            "case {case}: expected 400 {RESOURCE_IN_USE}, got {} body {}",
            reply.status,
            reply.body
        );
        let message = reply.text("Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(in_use_message(&group).as_str()),
            "case {case}: expected the request-derived refusal, got {message:?}"
        );
        // Nothing the STORE minted may reach the caller: not the load balancer, not a
        // listener, not a rule.
        let body = unescaped(&reply.body);
        for identifier in std::iter::once(lb.clone()).chain(minted.iter().cloned()) {
            assert!(
                !body.contains(identifier.as_str()),
                "case {case}: the refusal must not name {identifier}, got {body}"
            );
        }

        let still_there = describe(&router, &group).await;
        assert_eq!(
            first_text(&still_there.body, "TargetGroupArn").as_deref(),
            Some(group.as_str()),
            "case {case}: a refused delete must remove nothing, got {}",
            still_there.body
        );
    }
}

/// The refusal depends on the REQUEST, not on the store: two groups held by the same
/// listener are refused with two messages that differ only in the ARN each request
/// carried, and the group whose last reference goes is then deleted.
#[tokio::test]
async fn two_groups_of_one_listener_are_refused_with_their_own_arn_each() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "two groups", "inuse-pair-alb").await;
    let first = create_target_group(&router, "inuse-pair-tg-1", FILLER).await;
    let second = create_target_group(&router, "inuse-pair-tg-2", FILLER).await;
    let listener = create_listener(
        &router,
        "two groups",
        &lb,
        80,
        &[
            p("DefaultActions.member.1.Type", "forward"),
            p(
                "DefaultActions.member.1.ForwardConfig.TargetGroups.member.1.TargetGroupArn",
                &first,
            ),
            p(
                "DefaultActions.member.1.ForwardConfig.TargetGroups.member.2.TargetGroupArn",
                &second,
            ),
        ],
    )
    .await;
    // The ARN prefix of this service's own listeners, which no refusal below carries.
    let listener_prefix = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:listener/",
        account()
    );
    assert!(
        listener.starts_with(&listener_prefix),
        "case setup: expected a listener ARN under {listener_prefix}, got {listener}"
    );

    for (case, group) in [("the first group", &first), ("the second group", &second)] {
        let mut form = action("DeleteTargetGroup");
        form.push(p("TargetGroupArn", group));
        let reply = call(&router, &form).await;
        let message = reply.text("Message").map(|text| unescaped(&text));
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, RESOURCE_IN_USE),
            "case {case}: expected 400 {RESOURCE_IN_USE}, got {} body {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            message.as_deref(),
            Some(in_use_message(group).as_str()),
            "case {case}: expected the request-derived refusal, got {message:?}"
        );
    }

    // Releasing the one holder releases both: the refusal was about the reference, not
    // about the text.
    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", &listener));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case releasing the listener: expected 200, got {} body {}",
        deleted.status, deleted.body
    );
    for (case, group) in [("the first group", &first), ("the second group", &second)] {
        let mut form = action("DeleteTargetGroup");
        form.push(p("TargetGroupArn", group));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case} after the holder went: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let gone = describe(&router, group).await;
        assert_eq!(
            (gone.status, gone.error_code().as_str()),
            (400, TARGET_GROUP_NOT_FOUND),
            "case {case} after the holder went: it must be gone, got {}",
            gone.body
        );
    }
}
