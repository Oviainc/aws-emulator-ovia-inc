// SPDX-License-Identifier: CC0-1.0

//! What a reported RULE carries, driven over the wire exactly as a client drives it:
//! added with `CreateRule`, changed with `ModifyRule` and read back with `DescribeRules`.
//!
//! Two rules are asserted here, each over a SET of inputs rather than over the one call a
//! recording happens to carry:
//!
//! * `Rule$Transforms` is a list member every reported rule carries — the rules a caller
//!   added and the one its listener mints with itself alike — so a rule with no transform
//!   reports the member EMPTY rather than leaving it out, and a rule carrying transforms
//!   reports them all.
//! * A `fixed-response` action is reported with the `ContentType` the service defaulted
//!   when the request named none, and with the content type the request named when it did
//!   — wherever such an action is reported, a rule's actions and a listener's default
//!   actions included.
//!
//! The rows differ along what those rules generalise over: how many transforms a rule
//! carries and of which type, whether a fixed-response action names a content type and
//! which of the documented values it names, whether it names a message body, which
//! operation reports the rule (the create, the modification, a read by ARN and a read of
//! the whole listener) and whether the rule is one a caller added or the listener's own.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, fixed_response_configured, forward_to,
    host_header, host_header_rewrite, url_rewrite, Form,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p,
};
use crate::rule_deletion_and_condition_value_spellings::describe_rule;

/// The value `FixedResponseActionConfig$ContentType` takes when a fixed-response action
/// names none: "If the value is not specified, the default value is `text/plain`".
const DEFAULT_CONTENT_TYPE: &str = "text/plain";

/// Every content type a fixed-response action may name, from
/// `smithy_query(query="shape",shape="FixedResponseActionConfig")`'s `ContentType` member:
/// "Valid Values: text/plain | text/css | text/html | application/javascript |
/// application/json". The member is a plain string in the model, so this list is the
/// documented catalog rather than a modelled enum.
const CONTENT_TYPES: &[&str] = &[
    "text/plain",
    "text/css",
    "text/html",
    "application/javascript",
    "application/json",
];

/// One row of the rule table: the request fields the rule is created with, the transforms
/// it must then report and the content type its fixed-response action must report.
struct RuleRow {
    case: &'static str,
    priority: i32,
    fields: Form,
    /// The `Type` of every transform the rule must report, in order.
    transforms: Vec<&'static str>,
    /// The `FixedResponseConfig$ContentType` the rule's action must report, or `None`
    /// when the rule carries no fixed-response action at all.
    content_type: Option<&'static str>,
}

/// The `Type` of every transform one reported rule carries, in the order it reported them.
fn reported_transform_types(rule: &str) -> Vec<String> {
    members(rule, "Transforms")
        .iter()
        .map(|transform| first_text(transform, "Type").unwrap_or_default())
        .collect()
}

/// Whether one reported rule carries the `Transforms` member at all, which is a different
/// answer from carrying it empty (`<Transforms></Transforms>`).
fn carries_transforms_member(rule: &str) -> bool {
    rule.contains("<Transforms>") || rule.contains("<Transforms/>")
}

/// The `FixedResponseConfig$ContentType` of the first fixed-response action one reported
/// rule carries, or `None` when it carries no such action.
fn reported_content_type(rule: &str) -> Option<String> {
    members(rule, "Actions")
        .iter()
        .find(|rule_action| first_text(rule_action, "Type").as_deref() == Some("fixed-response"))
        .and_then(|rule_action| first_text(rule_action, "ContentType"))
}

/// Assert what one reported rule carries, whichever operation reported it.
fn assert_rule(case: &str, where_from: &str, rule: &str, row: &RuleRow) {
    assert!(
        carries_transforms_member(rule),
        "case {case}: {where_from} must report the Transforms member on every rule, got {rule}"
    );
    assert_eq!(
        reported_transform_types(rule),
        row.transforms,
        "case {case}: {where_from} expected transforms {:?}, got {rule}",
        row.transforms
    );
    assert_eq!(
        reported_content_type(rule).as_deref(),
        row.content_type,
        "case {case}: {where_from} expected content type {:?}, got {rule}",
        row.content_type
    );
}

/// The table every assertion below is made over: one row per combination of transforms and
/// fixed-response configuration, with a content type from each documented value and a row
/// that names none at all.
fn rule_table(target_group: &str) -> Vec<RuleRow> {
    let mut table = vec![
        RuleRow {
            case: "a forwarding rule with no transform at all",
            priority: 1,
            fields: [host_header("plain.example.com"), forward_to(target_group)].concat(),
            transforms: Vec::new(),
            content_type: None,
        },
        RuleRow {
            case: "a forwarding rule with one host header rewrite",
            priority: 2,
            fields: [
                host_header("one.example.com"),
                forward_to(target_group),
                host_header_rewrite(1, "^one\\.", "uno."),
            ]
            .concat(),
            transforms: vec!["host-header-rewrite"],
            content_type: None,
        },
        RuleRow {
            case: "a forwarding rule with one transform of each declared type",
            priority: 3,
            fields: [
                host_header("both.example.com"),
                forward_to(target_group),
                host_header_rewrite(1, "^both\\.", "due."),
                url_rewrite(2, "^/old/(.*)", "/new/$1"),
            ]
            .concat(),
            transforms: vec!["host-header-rewrite", "url-rewrite"],
            content_type: None,
        },
        RuleRow {
            case: "a fixed-response rule naming only the status code",
            priority: 10,
            fields: [
                host_header("status-only.example.com"),
                fixed_response_configured("503", None, None),
            ]
            .concat(),
            transforms: Vec::new(),
            content_type: Some(DEFAULT_CONTENT_TYPE),
        },
        RuleRow {
            case: "a fixed-response rule naming the status code and a message body",
            priority: 11,
            fields: [
                host_header("with-body.example.com"),
                fixed_response_configured("404", None, Some("gone from here")),
            ]
            .concat(),
            transforms: Vec::new(),
            content_type: Some(DEFAULT_CONTENT_TYPE),
        },
        RuleRow {
            case: "a fixed-response rule with no content type and a transform",
            priority: 12,
            fields: [
                host_header("defaulted-with-transform.example.com"),
                fixed_response_configured("200", None, Some("ok")),
                url_rewrite(1, "^/a/(.*)", "/b/$1"),
            ]
            .concat(),
            transforms: vec!["url-rewrite"],
            content_type: Some(DEFAULT_CONTENT_TYPE),
        },
    ];
    // One row per documented content type, including the one that is also the default, so
    // a value the request named is reported as named rather than as the default.
    for (index, content_type) in CONTENT_TYPES.iter().enumerate() {
        table.push(RuleRow {
            case: "a fixed-response rule naming one of the documented content types",
            priority: 100 + index as i32,
            fields: [
                host_header(&format!("named-{index}.example.com")),
                fixed_response_configured("200", Some(content_type), Some("body")),
            ]
            .concat(),
            transforms: Vec::new(),
            content_type: Some(content_type),
        });
    }
    table
}

/// Every rule a create, a read by ARN and a read of the whole listener reports carries the
/// `Transforms` member, and a fixed-response action's content type is the one the request
/// named or the one the service defaults.
#[tokio::test]
async fn reported_rules_carry_transforms_and_the_defaulted_content_type() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "rule-shape-tg").await;
    let lb = create_load_balancer(&router, "rule-shape-lb").await;
    let listener = create_listener(&router, &lb, &target_group, 80).await;

    let table = rule_table(&target_group);
    let mut arns = Vec::new();
    for row in &table {
        // The CREATE's own answer is the first place the rule is reported.
        let mut form = action("CreateRule");
        form.push(p("ListenerArn", &listener));
        form.push(p("Priority", &row.priority.to_string()));
        form.extend(row.fields.iter().cloned());
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200, got {} {}",
            row.case, reply.status, reply.body
        );
        let created = members(&reply.body, "Rules");
        assert_eq!(
            created.len(),
            1,
            "case {}: a create answers with the one rule it stored, got {}",
            row.case,
            reply.body
        );
        assert_rule(row.case, "CreateRule", &created[0], row);

        // What the write answered is what the store holds: the read by ARN is the
        // assertion the write's answer is only checked to agree with.
        let arn = first_text(&created[0], "RuleArn")
            .unwrap_or_else(|| panic!("case {}: no RuleArn in {}", row.case, reply.body));
        let read = describe_rule(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {}: reading the rule back expected 200, got {} {}",
            row.case, read.status, read.body
        );
        let stored = members(&read.body, "Rules");
        assert_eq!(
            stored.len(),
            1,
            "case {}: a read by ARN answers with the one rule it names, got {}",
            row.case,
            read.body
        );
        assert_rule(row.case, "DescribeRules by ARN", &stored[0], row);
        arns.push(arn);
    }

    // A read of the WHOLE listener reports the same rules the same way, and the rule the
    // listener mints with itself carries the member too.
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case the whole listener: expected 200, got {} {}",
        reply.status, reply.body
    );
    let reported = members(&reply.body, "Rules");
    assert_eq!(
        reported.len(),
        table.len() + 1,
        "case the whole listener: expected every rule plus the default one, got {}",
        reply.body
    );
    for (row, arn) in table.iter().zip(&arns) {
        let rule = reported
            .iter()
            .find(|rule| first_text(rule, "RuleArn").as_deref() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the listener must report the rule it holds, got {}",
                    row.case, reply.body
                )
            });
        assert_rule(row.case, "DescribeRules by listener", rule, row);
    }
    let default_rule = reported
        .iter()
        .find(|rule| first_text(rule, "IsDefault").as_deref() == Some("true"))
        .unwrap_or_else(|| {
            panic!(
                "case the default rule: every listener reports one, got {}",
                reply.body
            )
        });
    assert!(
        carries_transforms_member(default_rule),
        "case the default rule: it carries the Transforms member like every other rule, \
         got {default_rule}"
    );
    assert!(
        reported_transform_types(default_rule).is_empty(),
        "case the default rule: it carries no transform, got {default_rule}"
    );
}

/// A modification reports the rule it leaves behind exactly as a read of it does: the
/// `Transforms` member on every answer, and the content type the service defaulted on a
/// fixed-response action the modification put there.
#[tokio::test]
async fn a_modified_rule_reports_transforms_and_the_defaulted_content_type() {
    let (router, _engine) = emulator();
    let target_group = create_http_target_group(&router, "modify-shape-tg").await;
    let lb = create_load_balancer(&router, "modify-shape-lb").await;
    let listener = create_listener(&router, &lb, &target_group, 80).await;

    // Each row replaces the actions or the transforms of a rule created with a transform
    // and a forward, so the modification is asserted both where it removes a member's
    // contents and where it adds them.
    let rows: Vec<(&str, Form, Vec<&str>, Option<&str>)> = vec![
        (
            "the actions become a fixed-response naming no content type",
            fixed_response_configured("418", None, None),
            vec!["host-header-rewrite"],
            Some(DEFAULT_CONTENT_TYPE),
        ),
        (
            "the actions become a fixed-response naming a content type",
            fixed_response_configured("200", Some("application/json"), Some("{}")),
            vec!["host-header-rewrite"],
            Some("application/json"),
        ),
        (
            "every transform is removed and the fixed response stays",
            [
                fixed_response_configured("500", None, None),
                vec![p("ResetTransforms", "true")],
            ]
            .concat(),
            Vec::new(),
            Some(DEFAULT_CONTENT_TYPE),
        ),
    ];

    for (index, (case, fields, transforms, content_type)) in rows.iter().enumerate() {
        let arn = add_rule(
            &router,
            case,
            &listener,
            200 + index as i32,
            &[
                host_header(&format!("modify-{index}.example.com")),
                forward_to(&target_group),
                host_header_rewrite(1, "^modify", "modified"),
            ]
            .concat(),
        )
        .await;

        let mut form = action("ModifyRule");
        form.push(p("RuleArn", &arn));
        form.extend(fields.iter().cloned());
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let modified = members(&reply.body, "Rules");
        assert_eq!(
            modified.len(),
            1,
            "case {case}: a modification answers with the one rule it changed, got {}",
            reply.body
        );
        let row = RuleRow {
            case: "",
            priority: 0,
            fields: Vec::new(),
            transforms: transforms.clone(),
            content_type: *content_type,
        };
        assert_rule(case, "ModifyRule", &modified[0], &row);

        let read = describe_rule(&router, &arn).await;
        let stored = members(&read.body, "Rules");
        assert_eq!(
            stored.len(),
            1,
            "case {case}: a read by ARN answers with the one rule it names, got {}",
            read.body
        );
        assert_rule(case, "DescribeRules after the modification", &stored[0], &row);
    }
}

/// A listener's DEFAULT actions are reported through the same normalisation, so a
/// fixed-response default action carries the defaulted content type wherever it is
/// reported: on the create, on `DescribeListeners`, and on the default rule
/// `DescribeRules` answers with.
#[tokio::test]
async fn a_listener_default_fixed_response_reports_the_defaulted_content_type() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "default-action-lb").await;

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.extend(
        fixed_response_configured("404", None, Some("nothing here"))
            .into_iter()
            .map(|(key, value)| (key.replace("Actions.", "DefaultActions."), value)),
    );
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case a fixed-response default action: expected 200, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        first_text(&reply.body, "ContentType").as_deref(),
        Some(DEFAULT_CONTENT_TYPE),
        "case a fixed-response default action: CreateListener must report the defaulted \
         content type, got {}",
        reply.body
    );
    let listener = first_text(&reply.body, "ListenerArn").unwrap_or_else(|| {
        panic!(
            "case a fixed-response default action: no ListenerArn in {}",
            reply.body
        )
    });

    let mut form = action("DescribeListeners");
    form.push(p("ListenerArns.member.1", &listener));
    let described = call(&router, &form).await;
    assert_eq!(
        first_text(&described.body, "ContentType").as_deref(),
        Some(DEFAULT_CONTENT_TYPE),
        "case a fixed-response default action: DescribeListeners must report the defaulted \
         content type, got {}",
        described.body
    );

    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener));
    let rules = call(&router, &form).await;
    let default_rule = members(&rules.body, "Rules")
        .into_iter()
        .find(|rule| first_text(rule, "IsDefault").as_deref() == Some("true"))
        .unwrap_or_else(|| {
            panic!(
                "case a fixed-response default action: every listener reports a default \
                 rule, got {}",
                rules.body
            )
        });
    assert_eq!(
        reported_content_type(&default_rule).as_deref(),
        Some(DEFAULT_CONTENT_TYPE),
        "case a fixed-response default action: the default rule must report the defaulted \
         content type, got {default_rule}"
    );
    assert!(
        carries_transforms_member(&default_rule),
        "case a fixed-response default action: the default rule carries the Transforms \
         member, got {default_rule}"
    );
}
