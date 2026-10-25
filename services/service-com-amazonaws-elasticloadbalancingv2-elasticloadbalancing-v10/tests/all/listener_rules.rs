// SPDX-License-Identifier: CC0-1.0

//! A listener's RULES, driven over the wire exactly as a client drives them: added with
//! `CreateRule` and read back with `DescribeRules`.
//!
//! One rule is asserted here, over a SET of inputs rather than over the one call a
//! recording happens to carry: `DescribeRules` reports exactly the rules the store holds
//! for the selection the request names — the rules a caller added to the listener, in
//! evaluation order (lowest priority value first) with that listener's default rule
//! last — resolving a `RuleArns` selection to those same rules and refusing an entry
//! that names no rule.
//!
//! The rows differ along what that rule generalises over: the priority (including both
//! published bounds of `RulePriority`), the kind of condition (each documented field,
//! given directly and through its own configuration shape), the kind of action
//! (`forward`, `redirect`, `fixed-response`), the order the rules were created in, how
//! many rules a listener carries, the page size a read walks them with, and which of
//! them a selection names.
//!
//! Every accepted write is read back through `DescribeRules` rather than trusted from
//! the write's own answer, and after every refusal the listener's rules are read again
//! to prove nothing was stored.

use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `ListenerNotFoundException`
/// (`smithy_query(query="operation",operation="CreateRule")`: `awsQueryError` code
/// `ListenerNotFound`).
const LISTENER_NOT_FOUND: &str = "ListenerNotFound";

/// The awsQuery error code of `RuleNotFoundException`, which `DescribeRules` models.
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery error code of `PriorityInUseException`, which `CreateRule` models.
const PRIORITY_IN_USE: &str = "PriorityInUse";

/// The awsQuery error code of `TargetGroupNotFoundException`.
const TARGET_GROUP_NOT_FOUND: &str = "TargetGroupNotFound";

/// The error code this service answers an input its own documented rules refuse with.
const VALIDATION_ERROR: &str = "ValidationError";

/// The form fields of one awsQuery request.
type Form = Vec<(String, String)>;

/// One row of the rule table: the case, the priority the rule is evaluated at, the
/// request fields carrying its conditions and actions, and the values a read of it must
/// report back.
struct RuleRow {
    case: &'static str,
    priority: i32,
    fields: Form,
    reports: Vec<String>,
}

/// Create a load balancer across two subnets and answer its ARN.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create load balancer {name}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create load balancer {name}: no ARN in {}", reply.body))
}

/// Create an HTTP listener forwarding to `tg_arn` and answer its ARN.
async fn create_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    tg_arn: &str,
    port: i32,
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &port.to_string()));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener on port {port}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener on port {port}: no ARN in {}", reply.body))
}

/// Send one `CreateRule` request: the listener, the priority and `fields`.
async fn create_rule(
    router: &MultiServiceRouter,
    listener_arn: &str,
    priority: &str,
    fields: &[(String, String)],
) -> Reply {
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("Priority", priority));
    form.extend(fields.iter().cloned());
    call(router, &form).await
}

/// Add one rule that must be accepted, and answer its ARN.
async fn add_rule(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
    priority: i32,
    fields: &[(String, String)],
) -> String {
    let reply = create_rule(router, listener_arn, &priority.to_string(), fields).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200, got {} {}",
        reply.status, reply.body
    );
    let rules = members(&reply.body, "Rules");
    assert_eq!(
        rules.len(),
        1,
        "case {case}: a create answers with the one rule it stored, got {}",
        reply.body
    );
    assert_eq!(
        (
            first_text(&rules[0], "Priority").as_deref(),
            first_text(&rules[0], "IsDefault").as_deref()
        ),
        (Some(priority.to_string().as_str()), Some("false")),
        "case {case}: a created rule reports its own priority and is not the default, got {}",
        rules[0]
    );
    first_text(&rules[0], "RuleArn")
        .unwrap_or_else(|| panic!("case {case}: no RuleArn in {}", reply.body))
}

/// A forwarding action to `tg_arn`.
fn forward_to(tg_arn: &str) -> Form {
    vec![
        p("Actions.member.1.Type", "forward"),
        p("Actions.member.1.TargetGroupArn", tg_arn),
    ]
}

/// A fixed-response action answering `status`.
fn fixed_response(status: &str) -> Form {
    vec![
        p("Actions.member.1.Type", "fixed-response"),
        p("Actions.member.1.FixedResponseConfig.StatusCode", status),
        p("Actions.member.1.FixedResponseConfig.ContentType", "text/plain"),
    ]
}

/// One host-header condition matching `host`.
fn host_header(host: &str) -> Form {
    vec![
        p("Conditions.member.1.Field", "host-header"),
        p("Conditions.member.1.Values.member.1", host),
    ]
}

/// Read a listener's rules and answer `(priority, is_default)` for each, in the order
/// the read reported them.
async fn rules_of(router: &MultiServiceRouter, case: &str, listener_arn: &str) -> Vec<(String, String)> {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected 200, got {} {}",
        reply.status, reply.body
    );
    summaries(&reply)
}

/// `(Priority, IsDefault)` of every rule in one reply, in the order it reported them.
fn summaries(reply: &Reply) -> Vec<(String, String)> {
    members(&reply.body, "Rules")
        .iter()
        .map(|rule| {
            (
                first_text(rule, "Priority").unwrap_or_default(),
                first_text(rule, "IsDefault").unwrap_or_default(),
            )
        })
        .collect()
}

/// The `RuleArn` of every rule in one reply, in the order it reported them.
fn reported_arns(reply: &Reply) -> Vec<String> {
    members(&reply.body, "Rules")
        .iter()
        .map(|rule| first_text(rule, "RuleArn").unwrap_or_default())
        .collect()
}

/// The table of rules every ordering assertion below is made over: one row per kind of
/// condition the model documents, with the priorities spanning both published bounds of
/// `RulePriority` (range 1..50000).
fn rule_table(groups: &[String]) -> Vec<RuleRow> {
    vec![
        RuleRow {
            case: "a host header condition at the lowest published priority",
            priority: 1,
            fields: [
                host_header("shop.example.com"),
                forward_to(&groups[0]),
                // Tags live on the rule record; `DescribeTags` is what reads them back.
                vec![p("Tags.member.1.Key", "tier"), p("Tags.member.1.Value", "edge")],
            ]
            .concat(),
            reports: vec![
                "host-header".to_string(),
                "shop.example.com".to_string(),
                groups[0].clone(),
            ],
        },
        RuleRow {
            case: "a path pattern given through its own configuration shape",
            priority: 7,
            fields: [
                vec![
                    p("Conditions.member.1.Field", "path-pattern"),
                    p("Conditions.member.1.PathPatternConfig.Values.member.1", "/img/*"),
                    p("Conditions.member.1.PathPatternConfig.Values.member.2", "/static/*"),
                ],
                fixed_response("404"),
            ]
            .concat(),
            reports: vec![
                "path-pattern".to_string(),
                "/img/*".to_string(),
                "/static/*".to_string(),
                "404".to_string(),
            ],
        },
        RuleRow {
            case: "two conditions of the fields that may repeat, redirecting",
            priority: 40,
            fields: vec![
                p("Conditions.member.1.Field", "http-header"),
                p(
                    "Conditions.member.1.HttpHeaderConfig.HttpHeaderName",
                    "X-Tenant",
                ),
                p(
                    "Conditions.member.1.HttpHeaderConfig.Values.member.1",
                    "beta",
                ),
                p("Conditions.member.2.Field", "query-string"),
                p(
                    "Conditions.member.2.QueryStringConfig.Values.member.1.Key",
                    "mode",
                ),
                p(
                    "Conditions.member.2.QueryStringConfig.Values.member.1.Value",
                    "debug",
                ),
                p("Actions.member.1.Type", "redirect"),
                p("Actions.member.1.RedirectConfig.StatusCode", "HTTP_301"),
                p("Actions.member.1.RedirectConfig.Path", "/moved"),
            ],
            reports: vec![
                "http-header".to_string(),
                "X-Tenant".to_string(),
                "beta".to_string(),
                "query-string".to_string(),
                "mode".to_string(),
                "debug".to_string(),
                "HTTP_301".to_string(),
                "/moved".to_string(),
            ],
        },
        RuleRow {
            case: "a source ip condition forwarding to the second target group",
            priority: 500,
            fields: [
                vec![
                    p("Conditions.member.1.Field", "source-ip"),
                    p("Conditions.member.1.SourceIpConfig.Values.member.1", "10.0.0.0/8"),
                ],
                forward_to(&groups[1]),
            ]
            .concat(),
            reports: vec![
                "source-ip".to_string(),
                "10.0.0.0/8".to_string(),
                groups[1].clone(),
            ],
        },
        RuleRow {
            case: "an http request method condition at the highest published priority",
            priority: 50_000,
            fields: [
                vec![
                    p("Conditions.member.1.Field", "http-request-method"),
                    p(
                        "Conditions.member.1.HttpRequestMethodConfig.Values.member.1",
                        "POST",
                    ),
                ],
                fixed_response("503"),
            ]
            .concat(),
            reports: vec![
                "http-request-method".to_string(),
                "POST".to_string(),
                "503".to_string(),
            ],
        },
    ]
}

/// The rule: a listener's rules read back in evaluation order — lowest priority value
/// first, the default rule last — whatever order they were created in, each carrying
/// exactly what was persisted for it.
#[tokio::test]
async fn rules_read_back_in_priority_order_whatever_order_they_were_created() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-order-alb").await;
    let groups = [
        create_http_target_group(&router, "rule-order-tg-1").await,
        create_http_target_group(&router, "rule-order-tg-2").await,
    ];
    let listener = create_listener(&router, &lb, &groups[0], 80).await;
    let table = rule_table(&groups);

    // Created in an order that is neither the priority order nor its reverse, so the
    // order the read reports cannot be the order they were stored in.
    let creation_order = [2usize, 4, 0, 3, 1];
    let mut arn_of_priority: Vec<(i32, String)> = Vec::new();
    for index in creation_order {
        let row = &table[index];
        let arn = add_rule(&router, row.case, &listener, row.priority, &row.fields).await;
        arn_of_priority.push((row.priority, arn));
    }

    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case reading every rule of the listener: expected 200, got {} {}",
        reply.status, reply.body
    );
    let reported = summaries(&reply);
    let expected: Vec<(String, String)> = table
        .iter()
        .map(|row| (row.priority.to_string(), "false".to_string()))
        .chain(std::iter::once((
            "default".to_string(),
            "true".to_string(),
        )))
        .collect();
    assert_eq!(
        reported, expected,
        "case reading every rule of the listener: the rules come back in priority order with the default last, got {}",
        reply.body
    );

    // Every rule's ARN is its own, belongs to this listener, and is stable across reads.
    let arns = reported_arns(&reply);
    let prefix = format!("{}/", listener.replacen(":listener/", ":listener-rule/", 1));
    for arn in &arns {
        assert!(
            arn.starts_with(&prefix),
            "case every reported ARN: a rule of this listener carries the prefix {prefix}, got {arn}"
        );
    }
    let mut unique = arns.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        arns.len(),
        "case every reported ARN: each rule has its own, got {arns:?}"
    );
    assert_eq!(
        reported_arns(&call(&router, &form).await),
        arns,
        "case reading twice: the rules and their ARNs are stable across reads"
    );

    // What each create persisted is what a read of that one rule reports.
    for (priority, arn) in &arn_of_priority {
        let row = table
            .iter()
            .find(|row| row.priority == *priority)
            .expect("a row for every created rule");
        let mut form = action("DescribeRules");
        form.push(p("RuleArns.member.1", arn));
        let one = call(&router, &form).await;
        assert_eq!(
            one.status, 200,
            "case {}: reading the rule back by ARN expected 200, got {} {}",
            row.case, one.status, one.body
        );
        let rules = members(&one.body, "Rules");
        assert_eq!(
            rules.len(),
            1,
            "case {}: one ARN selects one rule, got {}",
            row.case,
            one.body
        );
        assert_eq!(
            first_text(&rules[0], "Priority").as_deref(),
            Some(row.priority.to_string().as_str()),
            "case {}: expected the stored priority, got {}",
            row.case,
            rules[0]
        );
        for value in &row.reports {
            assert!(
                rules[0].contains(value.as_str()),
                "case {}: the rule must report the persisted {value}, got {}",
                row.case,
                rules[0]
            );
        }
    }

    // The tags the lowest-priority rule was created with are stored on the rule, and
    // `DescribeTags` is the reader of a taggable resource's tags.
    let tagged = arn_of_priority
        .iter()
        .find(|(priority, _)| *priority == 1)
        .map(|(_, arn)| arn.clone())
        .expect("the rule created at priority 1");
    let mut form = action("DescribeTags");
    form.push(p("ResourceArns.member.1", &tagged));
    let tags = call(&router, &form).await;
    assert_eq!(
        (
            tags.status,
            first_text(&tags.body, "Key").as_deref(),
            first_text(&tags.body, "Value").as_deref()
        ),
        (200, Some("tier"), Some("edge")),
        "case the tags of a created rule: expected them read back, got {} {}",
        tags.status,
        tags.body
    );
    let mut form = action("DescribeTags");
    form.push(p("ResourceArns.member.1", &format!("{tagged}0")));
    let absent = call(&router, &form).await;
    assert_eq!(
        (absent.status, absent.error_code().as_str()),
        (400, RULE_NOT_FOUND),
        "case the tags of a rule ARN naming no rule: expected 400 {RULE_NOT_FOUND}, got {} {}",
        absent.status,
        absent.body
    );
}

/// The same rule, over the `RuleArns` selector: it resolves to the very rules the
/// listener holds, a repeat asks for one rule once, and an entry naming no rule OF the
/// selection is refused.
#[tokio::test]
async fn a_rule_arns_selection_resolves_to_the_rules_the_store_holds() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-select-alb").await;
    let group = create_http_target_group(&router, "rule-select-tg").await;
    let first = create_listener(&router, &lb, &group, 80).await;
    let second = create_listener(&router, &lb, &group, 8080).await;

    let mut on_first = Vec::new();
    for priority in [10i32, 20, 30] {
        on_first.push(
            add_rule(
                &router,
                "a rule of the first listener",
                &first,
                priority,
                &[host_header(&format!("p{priority}.example.com")), forward_to(&group)].concat(),
            )
            .await,
        );
    }
    let on_second = add_rule(
        &router,
        "a rule of the second listener",
        &second,
        10,
        &[host_header("other.example.com"), forward_to(&group)].concat(),
    )
    .await;

    // A subset selection answers exactly the rules it names.
    let mut form = action("DescribeRules");
    form.push(p("RuleArns.member.1", &on_first[2]));
    form.push(p("RuleArns.member.2", &on_first[0]));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case a subset selection: expected 200, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reported_arns(&reply),
        vec![on_first[2].clone(), on_first[0].clone()],
        "case a subset selection: expected the rules it named, got {}",
        reply.body
    );

    // Repeating an ARN asks for the same rule, which is resolved once.
    let mut form = action("DescribeRules");
    form.push(p("RuleArns.member.1", &on_first[1]));
    form.push(p("RuleArns.member.2", &on_first[1]));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reported_arns(&reply)),
        (200, vec![on_first[1].clone()]),
        "case a repeated ARN: expected the one rule once, got {}",
        reply.body
    );

    // A listener AND rules: those rules OF that listener.
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &first));
    form.push(p("RuleArns.member.1", &on_first[1]));
    let reply = call(&router, &form).await;
    assert_eq!(
        (reply.status, reported_arns(&reply)),
        (200, vec![on_first[1].clone()]),
        "case a listener and one of its rules: expected that rule, got {}",
        reply.body
    );

    // Each refusal of a selection, over the ways an entry can name no rule of it.
    let unknown = format!("{}0", on_first[0]);
    let refusals: Vec<(&str, Form, u16, &str)> = vec![
        (
            "a rule ARN naming no rule",
            vec![p("RuleArns.member.1", &unknown)],
            400,
            RULE_NOT_FOUND,
        ),
        (
            "a rule of another listener, selected under this one",
            vec![
                p("ListenerArn", &first),
                p("RuleArns.member.1", &on_second),
            ],
            400,
            RULE_NOT_FOUND,
        ),
        (
            "one known and one unknown rule",
            vec![
                p("RuleArns.member.1", &on_first[0]),
                p("RuleArns.member.2", &unknown),
            ],
            400,
            RULE_NOT_FOUND,
        ),
        ("neither a listener nor rules", Vec::new(), 400, VALIDATION_ERROR),
    ];
    for (case, fields, status, code) in refusals {
        let mut form = action("DescribeRules");
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (status, code),
            "case {case}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // A listener that is gone has no rules to report, and its rules go with it.
    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", &second));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case deleting the second listener: expected 200, got {} {}",
        deleted.status, deleted.body
    );
    for (case, fields) in [
        ("the deleted listener", vec![p("ListenerArn", &second)]),
        (
            "a rule of the deleted listener",
            vec![p("RuleArns.member.1", &on_second)],
        ),
    ] {
        let mut form = action("DescribeRules");
        form.extend(fields);
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400, got {} {}",
            reply.status, reply.body
        );
        let expected = if case == "the deleted listener" {
            LISTENER_NOT_FOUND
        } else {
            RULE_NOT_FOUND
        };
        assert_eq!(
            reply.error_code().as_str(),
            expected,
            "case {case}: expected {expected}, got {}",
            reply.body
        );
    }

    // The first listener kept every one of its rules.
    assert_eq!(
        rules_of(&router, "the surviving listener", &first).await.len(),
        4,
        "case the surviving listener: its three rules and its default rule remain"
    );
}

/// The same rule, walked one page at a time: every page size reports the same rules in
/// the same order, each exactly once.
#[tokio::test]
async fn rules_page_by_page_size_and_resume_from_the_marker() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-page-alb").await;
    let group = create_http_target_group(&router, "rule-page-tg").await;
    let listener = create_listener(&router, &lb, &group, 80).await;
    for priority in [4i32, 1, 3, 2] {
        add_rule(
            &router,
            "a rule to page over",
            &listener,
            priority,
            &[host_header(&format!("page{priority}.example.com")), forward_to(&group)].concat(),
        )
        .await;
    }

    let whole = rules_of(&router, "the whole collection", &listener).await;
    assert_eq!(
        whole
            .iter()
            .map(|(priority, _)| priority.as_str())
            .collect::<Vec<&str>>(),
        vec!["1", "2", "3", "4", "default"],
        "case the whole collection: expected every rule in priority order, got {whole:?}"
    );

    for page_size in 1..=whole.len() + 1 {
        let case = format!("PageSize={page_size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        // One more iteration than there can be pages, so a walk that never ends fails
        // the case instead of hanging.
        for _ in 0..whole.len() + 2 {
            let mut form = action("DescribeRules");
            form.push(p("ListenerArn", &listener));
            form.push(p("PageSize", &page_size.to_string()));
            if let Some(marker) = marker.as_deref() {
                form.push(p("Marker", marker));
            }
            let reply = call(&router, &form).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200, got {} {}",
                reply.status, reply.body
            );
            let page = summaries(&reply);
            assert!(
                page.len() <= page_size,
                "case {case}: a page holds at most {page_size} rules, got {page:?}"
            );
            walked.extend(page);
            marker = reply.text("NextMarker");
            if marker.is_none() {
                break;
            }
        }
        assert!(
            marker.is_none(),
            "case {case}: the walk must end with no NextMarker, got {marker:?}"
        );
        assert_eq!(
            walked, whole,
            "case {case}: paging reports every rule once, in the same order, got {walked:?}"
        );
    }

    // A marker that is no position in this collection is refused.
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener));
    form.push(p("Marker", "not-a-marker"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 400,
        "case an invalid marker: expected 400, got {} {}",
        reply.status, reply.body
    );
}

/// What `CreateRule` refuses, and that a refused create stores nothing — so the rules a
/// later read reports are exactly the ones it accepted.
#[tokio::test]
async fn a_refused_rule_is_not_stored_and_a_priority_is_per_listener() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-refuse-alb").await;
    let group = create_http_target_group(&router, "rule-refuse-tg").await;
    let listener = create_listener(&router, &lb, &group, 80).await;
    let other = create_listener(&router, &lb, &group, 8080).await;
    let gone = create_listener(&router, &lb, &group, 8081).await;
    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", &gone));
    call(&router, &form).await;

    add_rule(
        &router,
        "the rule whose priority is then in use",
        &listener,
        10,
        &[host_header("taken.example.com"), forward_to(&group)].concat(),
    )
    .await;
    // The priority lives in its listener's own namespace: the same number elsewhere is
    // free.
    add_rule(
        &router,
        "the same priority on another listener",
        &other,
        10,
        &[host_header("free.example.com"), forward_to(&group)].concat(),
    )
    .await;
    // Five values in one rule's conditions is the published bound, and is accepted.
    let five_values: Form = (1..=5)
        .map(|index| {
            p(
                &format!("Conditions.member.1.Values.member.{index}"),
                &format!("v{index}.example.com"),
            )
        })
        .collect();
    add_rule(
        &router,
        "five condition values, the published bound",
        &listener,
        11,
        &[
            vec![p("Conditions.member.1.Field", "host-header")],
            five_values.clone(),
            forward_to(&group),
        ]
        .concat(),
    )
    .await;

    let before = rules_of(&router, "before any refusal", &listener).await;
    let six_values: Form = (1..=6)
        .map(|index| {
            p(
                &format!("Conditions.member.1.Values.member.{index}"),
                &format!("w{index}.example.com"),
            )
        })
        .collect();

    // Each row: the case, the listener the rule is asked for, the priority, the request
    // fields, and the expected error code ("" for a refusal whose code is the boundary
    // validator's rather than this service's).
    let refusals: Vec<(&str, &str, &str, Form, &str)> = vec![
        (
            "a listener that was deleted",
            &gone,
            "1",
            [host_header("a.example.com"), forward_to(&group)].concat(),
            LISTENER_NOT_FOUND,
        ),
        (
            "a value that is no listener ARN",
            "not-a-listener-arn",
            "1",
            [host_header("a.example.com"), forward_to(&group)].concat(),
            VALIDATION_ERROR,
        ),
        (
            "a priority already in use on this listener",
            &listener,
            "10",
            [host_header("b.example.com"), forward_to(&group)].concat(),
            PRIORITY_IN_USE,
        ),
        (
            "no conditions at all",
            &listener,
            "20",
            forward_to(&group),
            "",
        ),
        (
            "a condition field this service does not match on",
            &listener,
            "21",
            [
                vec![
                    p("Conditions.member.1.Field", "hostheader"),
                    p("Conditions.member.1.Values.member.1", "c.example.com"),
                ],
                forward_to(&group),
            ]
            .concat(),
            VALIDATION_ERROR,
        ),
        (
            "two conditions of a field that may appear once",
            &listener,
            "22",
            [
                host_header("d.example.com"),
                vec![
                    p("Conditions.member.2.Field", "host-header"),
                    p("Conditions.member.2.Values.member.1", "e.example.com"),
                ],
                forward_to(&group),
            ]
            .concat(),
            VALIDATION_ERROR,
        ),
        (
            "six condition values, one past the published bound",
            &listener,
            "23",
            [
                vec![p("Conditions.member.1.Field", "host-header")],
                six_values,
                forward_to(&group),
            ]
            .concat(),
            VALIDATION_ERROR,
        ),
        (
            "a forward to a target group that does not exist",
            &listener,
            "24",
            [
                host_header("f.example.com"),
                forward_to(&format!("{group}0")),
            ]
            .concat(),
            TARGET_GROUP_NOT_FOUND,
        ),
        (
            "a priority below the published range",
            &listener,
            "0",
            [host_header("g.example.com"), forward_to(&group)].concat(),
            "",
        ),
        (
            "a priority above the published range",
            &listener,
            "50001",
            [host_header("h.example.com"), forward_to(&group)].concat(),
            "",
        ),
        (
            "no actions at all",
            &listener,
            "25",
            host_header("i.example.com"),
            "",
        ),
    ];

    for (case, listener_arn, priority, fields, code) in refusals {
        let reply = create_rule(&router, listener_arn, priority, &fields).await;
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
        assert_eq!(
            rules_of(&router, case, &listener).await,
            before,
            "case {case}: a refused create must store no rule"
        );
    }

    // The target group both listeners' rules forward to reports the load balancer that
    // routes to it through them.
    let mut form = action("DescribeTargetGroups");
    form.push(p("TargetGroupArns.member.1", &group));
    let groups = call(&router, &form).await;
    assert!(
        groups.body.contains(lb.as_str()),
        "case the forwarded target group: it must report the load balancer routing to it, got {}",
        groups.body
    );
}

/// How many rules one load balancer holds across its listeners before `CreateRule`
/// refuses: the figure this service's own `DescribeAccountLimits` publishes for
/// `rules-per-application-load-balancer`.
const RULES_PER_LOAD_BALANCER: usize = 100;

/// The awsQuery error code of `TooManyRulesException`.
const TOO_MANY_RULES: &str = "TooManyRules";

/// The bound on the rules one LOAD BALANCER holds, which is counted across its
/// listeners: on the bound a create is accepted, one past it is the modelled quota
/// refusal, and the refusal stores nothing.
#[tokio::test]
async fn the_rules_of_one_load_balancer_are_bounded_across_its_listeners() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-quota-alb").await;
    let group = create_http_target_group(&router, "rule-quota-tg").await;
    let first = create_listener(&router, &lb, &group, 80).await;
    let second = create_listener(&router, &lb, &group, 8080).await;

    // Split across two listeners of the same load balancer, because the bound is the
    // load balancer's and not one listener's.
    for index in 0..RULES_PER_LOAD_BALANCER {
        let listener = if index % 2 == 0 { &first } else { &second };
        add_rule(
            &router,
            "a rule up to the published bound",
            listener,
            index as i32 + 1,
            &[
                host_header(&format!("q{index}.example.com")),
                forward_to(&group),
            ]
            .concat(),
        )
        .await;
    }
    let before = (
        rules_of(&router, "the first listener on the bound", &first).await,
        rules_of(&router, "the second listener on the bound", &second).await,
    );
    assert_eq!(
        before.0.len() + before.1.len(),
        RULES_PER_LOAD_BALANCER + 2,
        "case on the bound: every rule plus each listener's default rule must be stored"
    );

    for (case, listener) in [
        ("one rule past the bound, on the first listener", &first),
        ("one rule past the bound, on the second listener", &second),
    ] {
        let reply = create_rule(
            &router,
            listener,
            "50000",
            &[host_header("past.example.com"), forward_to(&group)].concat(),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, TOO_MANY_RULES),
            "case {case}: expected 400 {TOO_MANY_RULES}, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            (
                rules_of(&router, case, &first).await,
                rules_of(&router, case, &second).await
            ),
            before,
            "case {case}: a refused create must store no rule"
        );
    }

    // The bound is this load balancer's: another one starts from nothing.
    let other_lb = create_load_balancer(&router, "rule-quota-alb-2").await;
    let other_listener = create_listener(&router, &other_lb, &group, 80).await;
    add_rule(
        &router,
        "the first rule of another load balancer",
        &other_listener,
        1,
        &[host_header("fresh.example.com"), forward_to(&group)].concat(),
    )
    .await;
}

/// The awsQuery error code of `ResourceInUseException`, which `DeleteTargetGroup`
/// models.
const RESOURCE_IN_USE: &str = "ResourceInUse";

/// A target group is referenced by an ACTION whether that action is a listener's default
/// action or one of its RULES' actions — "you can delete a target group if it is not
/// referenced by any actions" — so a rule keeps the target group it forwards to alive
/// exactly as a default action does, and releasing the rule releases it.
#[tokio::test]
async fn a_target_group_a_rule_forwards_to_is_in_use() {
    let (router, _engine) = emulator();
    let lb = create_load_balancer(&router, "rule-inuse-alb").await;
    let by_default = create_http_target_group(&router, "rule-inuse-tg-1").await;
    let by_rule = create_http_target_group(&router, "rule-inuse-tg-2").await;
    let listener = create_listener(&router, &lb, &by_default, 80).await;
    add_rule(
        &router,
        "the rule that forwards to the second target group",
        &listener,
        10,
        &[host_header("rule.example.com"), forward_to(&by_rule)].concat(),
    )
    .await;

    for (case, target_group) in [
        ("the target group a default action forwards to", &by_default),
        ("the target group a rule forwards to", &by_rule),
    ] {
        let mut form = action("DeleteTargetGroup");
        form.push(p("TargetGroupArn", target_group));
        let reply = call(&router, &form).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, RESOURCE_IN_USE),
            "case {case}: expected 400 {RESOURCE_IN_USE}, got {} {}",
            reply.status,
            reply.body
        );
        assert!(
            reply.body.contains(listener.as_str()),
            "case {case}: the refusal names the listener referencing it, got {}",
            reply.body
        );
        let mut form = action("DescribeTargetGroups");
        form.push(p("TargetGroupArns.member.1", target_group));
        let still = call(&router, &form).await;
        assert_eq!(
            still.status, 200,
            "case {case}: a refused delete must leave the target group, got {} {}",
            still.status, still.body
        );
    }

    // Releasing the listener releases its rules, and with them the reference.
    let mut form = action("DeleteListener");
    form.push(p("ListenerArn", &listener));
    let deleted = call(&router, &form).await;
    assert_eq!(
        deleted.status, 200,
        "case deleting the listener: expected 200, got {} {}",
        deleted.status, deleted.body
    );
    for (case, target_group) in [
        ("the target group of the released default action", &by_default),
        ("the target group of the released rule", &by_rule),
    ] {
        let mut form = action("DeleteTargetGroup");
        form.push(p("TargetGroupArn", target_group));
        let reply = call(&router, &form).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: nothing references it any more, so expected 200, got {} {}",
            reply.status, reply.body
        );
        let mut form = action("DescribeTargetGroups");
        form.push(p("TargetGroupArns.member.1", target_group));
        let gone = call(&router, &form).await;
        assert_eq!(
            (gone.status, gone.error_code().as_str()),
            (400, TARGET_GROUP_NOT_FOUND),
            "case {case}: the deleted target group must be gone, got {} {}",
            gone.status,
            gone.body
        );
    }
}
