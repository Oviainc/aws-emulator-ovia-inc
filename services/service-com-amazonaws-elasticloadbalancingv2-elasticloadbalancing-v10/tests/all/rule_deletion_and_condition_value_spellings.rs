// SPDX-License-Identifier: CC0-1.0

//! Tearing a listener RULE down, and the spellings a rule condition's values are read
//! back under — both driven over the wire exactly as a client drives them.
//!
//! Two rules are asserted here, each over a SET of inputs rather than over the one call a
//! recording happens to carry:
//!
//! * `DeleteRule` releases exactly the rule its `RuleArn` names from the listener that
//!   holds it — the read stops reporting it, every other rule of that listener and of its
//!   siblings is untouched, and a target group only that rule's actions forwarded to
//!   stops being referenced — while the rule a listener mints with itself is refused
//!   (`OperationNotPermitted`) and a `RuleArn` the store holds no rule for is
//!   `RuleNotFound`.
//! * A condition is reported under every spelling the model gives its field: the
//!   top-level `Values` for `host-header` and `path-pattern`, the top-level `RegexValues`
//!   for those two and `http-header`, each carrying the same array as the field's own
//!   configuration shape — and a member the field has no top-level spelling for is
//!   ABSENT, never an empty list.
//!
//! The rows differ along what those rules generalise over: which rule of several is
//! deleted and in what order, the kind of condition and of action the deleted rule
//! carried, whether the rule forwarded to a target group, and — for the condition rule —
//! the field, the spelling the create used (top-level or configuration), the number of
//! values and whether they are patterns or regular expressions.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, create_rule, fixed_response, forward_to,
    host_header, rules_of, Form,
};
use crate::provisioning_loop::{
    action, call, create_http_target_group, emulator, first_text, members, p, Reply,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `RuleNotFoundException`, which `DeleteRule` models
/// (`smithy_query(query="operation",operation="DeleteRule")`: `awsQueryError` code
/// `RuleNotFound`).
const RULE_NOT_FOUND: &str = "RuleNotFound";

/// The awsQuery error code of `OperationNotPermittedException`, the other error
/// `DeleteRule` models.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery error code of `ResourceInUseException`, which `DeleteTargetGroup` models.
const RESOURCE_IN_USE: &str = "ResourceInUse";

/// The configuration elements a `RuleCondition` can carry, from the model's own member
/// list (`smithy_query(query="shape",shape="RuleCondition")`). Stripping them is what
/// leaves a condition's TOP-LEVEL members, so a `Values` inside a configuration shape is
/// never read as the legacy one.
const CONFIG_ELEMENTS: &[&str] = &[
    "HostHeaderConfig",
    "PathPatternConfig",
    "HttpHeaderConfig",
    "QueryStringConfig",
    "HttpRequestMethodConfig",
    "SourceIpConfig",
];

/// One row of the delete table: the rule to add, and whether it is the only thing
/// referencing the target group set aside for the rules.
struct DeleteRow {
    case: &'static str,
    priority: i32,
    fields: Form,
    forwards_to_the_rule_target_group: bool,
}

/// One row of the condition table: the condition to create, and what a read of it must
/// report under each spelling.
struct ConditionRow {
    case: &'static str,
    priority: i32,
    fields: Form,
    /// The `Field` the read must report.
    field: &'static str,
    /// The top-level `Values` the read must report, or `None` when that member must be
    /// ABSENT from the condition.
    values: Option<&'static [&'static str]>,
    /// The top-level `RegexValues`, read the same way.
    regex_values: Option<&'static [&'static str]>,
    /// The field's own configuration element, and the values it must still report.
    config: &'static str,
    config_values: &'static [&'static str],
}

/// Send one `DeleteRule` request.
async fn delete_rule(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DeleteRule");
    form.push(p("RuleArn", arn));
    call(router, &form).await
}

/// Send one `DeleteTargetGroup` request.
async fn delete_target_group(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DeleteTargetGroup");
    form.push(p("TargetGroupArn", arn));
    call(router, &form).await
}

/// Read ONE rule back by its ARN.
async fn describe_rule(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DescribeRules");
    form.push(p("RuleArns.member.1", arn));
    call(router, &form).await
}

/// The ARN of the rule a listener mints with itself, as `DescribeRules` reports it.
async fn default_rule_arn(router: &MultiServiceRouter, listener_arn: &str) -> String {
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", listener_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "Rules")
        .into_iter()
        .find(|rule| first_text(rule, "IsDefault").as_deref() == Some("true"))
        .and_then(|rule| first_text(&rule, "RuleArn"))
        .unwrap_or_else(|| {
            panic!(
                "case the default rule of a listener: every listener reports one, got {}",
                reply.body
            )
        })
}

/// One condition's TOP-LEVEL members: the condition with every configuration element cut
/// out of it, so what is left is only what the condition itself spells.
fn outside_configs(condition: &str) -> String {
    let mut out = condition.to_string();
    for name in CONFIG_ELEMENTS {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        while let Some(start) = out.find(&open) {
            let end = match out[start..].find(&close) {
                Some(at) => start + at + close.len(),
                None => out.len(),
            };
            out.replace_range(start..end, "");
        }
    }
    out
}

/// The XML inside the first element named `name`, or `None` when there is none. A
/// configuration element never nests an element of its own name, so this needs no depth
/// tracking.
fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// Assert what one top-level list member of a condition reports: the exact array when the
/// field has that spelling, and nothing at all when it has none.
fn check_top_level(
    case: &str,
    source: &str,
    name: &str,
    top_level: &str,
    expected: Option<&[&str]>,
) {
    let present = top_level.contains(&format!("<{name}"));
    match expected {
        None => assert!(
            !present,
            "case {case} ({source}): a field with no top-level {name} spelling must report \
             none at all, got {top_level}"
        ),
        Some(values) => {
            let reported = members(top_level, name);
            let wanted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
            assert_eq!(
                reported, wanted,
                "case {case} ({source}): expected the top-level {name} {wanted:?}, got \
                 {reported:?} in {top_level}"
            );
        }
    }
}

/// The rules the delete table adds: one row per kind of rule a caller adds, spanning the
/// condition fields and action kinds, at priorities across the published `RulePriority`
/// range, in ascending priority order.
fn delete_table(rule_target_group: &str) -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "a host header rule forwarding to its own target group",
            priority: 1,
            fields: [host_header("shop.example.com"), forward_to(rule_target_group)].concat(),
            forwards_to_the_rule_target_group: true,
        },
        DeleteRow {
            case: "a path pattern rule given through its configuration shape",
            priority: 7,
            fields: [
                vec![
                    p("Conditions.member.1.Field", "path-pattern"),
                    p("Conditions.member.1.PathPatternConfig.Values.member.1", "/img/*"),
                ],
                fixed_response("404"),
            ]
            .concat(),
            forwards_to_the_rule_target_group: false,
        },
        DeleteRow {
            case: "an http header rule redirecting",
            priority: 100,
            fields: vec![
                p("Conditions.member.1.Field", "http-header"),
                p("Conditions.member.1.HttpHeaderConfig.HttpHeaderName", "X-Tenant"),
                p("Conditions.member.1.HttpHeaderConfig.Values.member.1", "beta"),
                p("Actions.member.1.Type", "redirect"),
                p("Actions.member.1.RedirectConfig.StatusCode", "HTTP_301"),
                p("Actions.member.1.RedirectConfig.Path", "/moved"),
            ],
            forwards_to_the_rule_target_group: false,
        },
        DeleteRow {
            case: "a source ip rule at the highest published priority",
            priority: 50_000,
            fields: [
                vec![
                    p("Conditions.member.1.Field", "source-ip"),
                    p("Conditions.member.1.SourceIpConfig.Values.member.1", "10.0.0.0/8"),
                ],
                fixed_response("503"),
            ]
            .concat(),
            forwards_to_the_rule_target_group: false,
        },
    ]
}

/// The rule: `DeleteRule` releases exactly the rule its `RuleArn` names, whatever order
/// the rules were added or deleted in; the rule a listener mints with itself cannot be
/// deleted; and an ARN naming no rule — one already deleted included — is refused.
#[tokio::test]
async fn deleting_a_rule_releases_only_that_rule_and_never_the_default_rule() {
    let (router, _engine) = emulator();
    let load_balancer = create_load_balancer(&router, "rule-delete-alb").await;
    let listener_group = create_http_target_group(&router, "rule-delete-listener-tg").await;
    let rule_group = create_http_target_group(&router, "rule-delete-rule-tg").await;
    let listener = create_listener(&router, &load_balancer, &listener_group, 80).await;
    let sibling_listener = create_listener(&router, &load_balancer, &listener_group, 8080).await;

    let table = delete_table(&rule_group);
    let mut arns: Vec<String> = Vec::new();
    for row in &table {
        arns.push(add_rule(&router, row.case, &listener, row.priority, &row.fields).await);
    }
    let sibling_rule = add_rule(
        &router,
        "a rule of the sibling listener",
        &sibling_listener,
        10,
        &[host_header("sibling.example.com"), fixed_response("404")].concat(),
    )
    .await;

    // The target group only a RULE forwards to is in use for exactly as long as that rule
    // exists, which is what the deletes below are checked against.
    let in_use = delete_target_group(&router, &rule_group).await;
    assert_eq!(
        (in_use.status, in_use.error_code().as_str()),
        (400, RESOURCE_IN_USE),
        "case the target group a rule forwards to: expected 400 {RESOURCE_IN_USE} while the \
         rule exists, got {} {}",
        in_use.status,
        in_use.body
    );

    // Deleted in an order that is neither the creation order nor the priority order, so
    // what each delete removes cannot be an artefact of position.
    let order = [1usize, 3, 0, 2];
    let mut live: Vec<usize> = (0..table.len()).collect();
    let mut rule_group_released = false;
    for index in order {
        let row = &table[index];
        let arn = &arns[index];

        let reply = delete_rule(&router, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200, got {} {}",
            row.case, reply.status, reply.body
        );
        assert!(
            reply.body.contains("DeleteRuleResponse") && !reply.body.contains("<Rules"),
            "case {}: a delete answers an empty DeleteRuleOutput, got {}",
            row.case,
            reply.body
        );

        live.retain(|at| *at != index);
        let expected: Vec<(String, String)> = live
            .iter()
            .map(|at| (table[*at].priority.to_string(), "false".to_string()))
            .chain(std::iter::once((
                "default".to_string(),
                "true".to_string(),
            )))
            .collect();
        let reported = rules_of(&router, row.case, &listener).await;
        assert_eq!(
            reported, expected,
            "case {}: the listener must report exactly the rules that are left, in priority \
             order with the default last",
            row.case
        );

        // The deleted rule is gone for good: reading it and deleting it again are both
        // the modelled refusal for a rule the store does not hold.
        let read = describe_rule(&router, arn).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (400, RULE_NOT_FOUND),
            "case {}: reading a deleted rule expected 400 {RULE_NOT_FOUND}, got {} {}",
            row.case,
            read.status,
            read.body
        );
        let again = delete_rule(&router, arn).await;
        assert_eq!(
            (again.status, again.error_code().as_str()),
            (400, RULE_NOT_FOUND),
            "case {}: deleting it twice expected 400 {RULE_NOT_FOUND}, got {} {}",
            row.case,
            again.status,
            again.body
        );

        // Nothing else keeps the rules' target group alive, so the delete that releases
        // the last rule forwarding to it is the one that lets it be deleted.
        if !rule_group_released {
            let attempt = delete_target_group(&router, &rule_group).await;
            if row.forwards_to_the_rule_target_group {
                assert_eq!(
                    attempt.status, 200,
                    "case {}: with the forwarding rule gone the target group is free, got {} {}",
                    row.case, attempt.status, attempt.body
                );
                rule_group_released = true;
            } else {
                assert_eq!(
                    (attempt.status, attempt.error_code().as_str()),
                    (400, RESOURCE_IN_USE),
                    "case {}: this delete does not release the target group, expected 400 \
                     {RESOURCE_IN_USE}, got {} {}",
                    row.case,
                    attempt.status,
                    attempt.body
                );
            }
        }
    }
    assert!(
        rule_group_released,
        "case the target group a rule forwarded to: deleting that rule must release it"
    );

    // The target group the LISTENER's own default action forwards to is still in use,
    // whatever happened to the rules.
    let still_used = delete_target_group(&router, &listener_group).await;
    assert_eq!(
        (still_used.status, still_used.error_code().as_str()),
        (400, RESOURCE_IN_USE),
        "case the target group the listener's default action forwards to: expected 400 \
         {RESOURCE_IN_USE}, got {} {}",
        still_used.status,
        still_used.body
    );

    // A rule of another listener is another listener's: no delete above touched it.
    assert_eq!(
        rules_of(&router, "the sibling listener", &sibling_listener).await,
        vec![
            ("10".to_string(), "false".to_string()),
            ("default".to_string(), "true".to_string()),
        ],
        "case the sibling listener: its own rule survives every delete on the first listener"
    );
    let sibling_read = describe_rule(&router, &sibling_rule).await;
    assert_eq!(
        sibling_read.status, 200,
        "case the sibling listener's rule: it still reads back, got {} {}",
        sibling_read.status, sibling_read.body
    );

    // "You can't delete the default rule": it is refused, and it is still there after.
    let default_rule = default_rule_arn(&router, &listener).await;
    let refused = delete_rule(&router, &default_rule).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, OPERATION_NOT_PERMITTED),
        "case the default rule of a listener: expected 400 {OPERATION_NOT_PERMITTED}, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        rules_of(&router, "after refusing the default rule", &listener).await,
        vec![("default".to_string(), "true".to_string())],
        "case the default rule of a listener: a refused delete removes nothing"
    );

    // An ARN this store holds no rule for at all, well formed and naming the listener.
    let unknown = format!(
        "{}/0123456789abcdef",
        listener.replacen(":listener/", ":listener-rule/", 1)
    );
    let missing = delete_rule(&router, &unknown).await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (400, RULE_NOT_FOUND),
        "case an ARN naming no rule: expected 400 {RULE_NOT_FOUND}, got {} {}",
        missing.status,
        missing.body
    );
}

/// The condition table: one row per spelling a condition's values can be created under,
/// spanning every documented condition field, in ascending priority order.
///
/// The expectations come from the model's own documentation of the two top-level members
/// (`smithy_query(query="shape",shape="RuleCondition")`): `Values` is to be specified
/// "only when Field is host-header or path-pattern. Alternatively … use HostHeaderConfig
/// or PathPatternConfig", and `RegexValues` "only when Field is http-header, host-header,
/// or path-pattern".
fn condition_table() -> Vec<ConditionRow> {
    vec![
        ConditionRow {
            case: "a path pattern given only through its configuration shape",
            priority: 1,
            fields: vec![
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.PathPatternConfig.Values.member.1", "/only-config"),
                p("Conditions.member.1.PathPatternConfig.Values.member.2", "/img/*"),
            ],
            field: "path-pattern",
            values: Some(&["/only-config", "/img/*"]),
            regex_values: None,
            config: "PathPatternConfig",
            config_values: &["/only-config", "/img/*"],
        },
        ConditionRow {
            case: "a path pattern given only in the legacy top-level Values",
            priority: 2,
            fields: vec![
                p("Conditions.member.1.Field", "path-pattern"),
                p("Conditions.member.1.Values.member.1", "/legacy-only"),
            ],
            field: "path-pattern",
            values: Some(&["/legacy-only"]),
            regex_values: None,
            config: "PathPatternConfig",
            config_values: &["/legacy-only"],
        },
        ConditionRow {
            case: "two host names given only through the configuration shape",
            priority: 3,
            fields: vec![
                p("Conditions.member.1.Field", "host-header"),
                p("Conditions.member.1.HostHeaderConfig.Values.member.1", "a.example.com"),
                p("Conditions.member.1.HostHeaderConfig.Values.member.2", "b.example.com"),
            ],
            field: "host-header",
            values: Some(&["a.example.com", "b.example.com"]),
            regex_values: None,
            config: "HostHeaderConfig",
            config_values: &["a.example.com", "b.example.com"],
        },
        ConditionRow {
            case: "one host name given only in the legacy top-level Values",
            priority: 4,
            fields: vec![
                p("Conditions.member.1.Field", "host-header"),
                p("Conditions.member.1.Values.member.1", "single.example.com"),
            ],
            field: "host-header",
            values: Some(&["single.example.com"]),
            regex_values: None,
            config: "HostHeaderConfig",
            config_values: &["single.example.com"],
        },
        ConditionRow {
            case: "an http header, a field with no top-level Values spelling",
            priority: 5,
            fields: vec![
                p("Conditions.member.1.Field", "http-header"),
                p("Conditions.member.1.HttpHeaderConfig.HttpHeaderName", "X-Probe"),
                p("Conditions.member.1.HttpHeaderConfig.Values.member.1", "probe"),
            ],
            field: "http-header",
            values: None,
            regex_values: None,
            config: "HttpHeaderConfig",
            config_values: &["X-Probe", "probe"],
        },
        ConditionRow {
            case: "a query string parameter, a field with no top-level spelling at all",
            priority: 6,
            fields: vec![
                p("Conditions.member.1.Field", "query-string"),
                p("Conditions.member.1.QueryStringConfig.Values.member.1.Key", "mode"),
                p("Conditions.member.1.QueryStringConfig.Values.member.1.Value", "debug"),
            ],
            field: "query-string",
            values: None,
            regex_values: None,
            config: "QueryStringConfig",
            config_values: &["mode", "debug"],
        },
        ConditionRow {
            case: "an http request method, a field with no top-level spelling at all",
            priority: 7,
            fields: vec![
                p("Conditions.member.1.Field", "http-request-method"),
                p("Conditions.member.1.HttpRequestMethodConfig.Values.member.1", "POST"),
            ],
            field: "http-request-method",
            values: None,
            regex_values: None,
            config: "HttpRequestMethodConfig",
            config_values: &["POST"],
        },
        ConditionRow {
            case: "a source ip range, a field with no top-level spelling at all",
            priority: 8,
            fields: vec![
                p("Conditions.member.1.Field", "source-ip"),
                p("Conditions.member.1.SourceIpConfig.Values.member.1", "10.0.0.0/8"),
            ],
            field: "source-ip",
            values: None,
            regex_values: None,
            config: "SourceIpConfig",
            config_values: &["10.0.0.0/8"],
        },
        ConditionRow {
            case: "path pattern regular expressions given only through the configuration shape",
            priority: 9,
            fields: vec![
                p("Conditions.member.1.Field", "path-pattern"),
                p(
                    "Conditions.member.1.PathPatternConfig.RegexValues.member.1",
                    "^/api/v[0-9]+$",
                ),
            ],
            field: "path-pattern",
            // No value was given under either spelling, so the condition reports none:
            // mirroring fills in a spelling that was left out, it never invents an array.
            values: None,
            regex_values: Some(&["^/api/v[0-9]+$"]),
            config: "PathPatternConfig",
            config_values: &["^/api/v[0-9]+$"],
        },
        ConditionRow {
            case: "http header regular expressions, a field whose regular expressions have a \
                   top-level spelling but whose values do not",
            priority: 10,
            fields: vec![
                p("Conditions.member.1.Field", "http-header"),
                p("Conditions.member.1.HttpHeaderConfig.HttpHeaderName", "X-Version"),
                p(
                    "Conditions.member.1.HttpHeaderConfig.RegexValues.member.1",
                    "^v[0-9]+$",
                ),
            ],
            field: "http-header",
            values: None,
            regex_values: Some(&["^v[0-9]+$"]),
            config: "HttpHeaderConfig",
            config_values: &["X-Version", "^v[0-9]+$"],
        },
    ]
}

/// The rule: a rule condition reads back under every spelling the model gives its field —
/// the same array under the legacy top-level member and under the field's configuration
/// shape — and a member the field has no top-level spelling for is absent from it.
#[tokio::test]
async fn a_condition_reports_its_values_under_every_spelling_its_field_has() {
    let (router, _engine) = emulator();
    let load_balancer = create_load_balancer(&router, "rule-values-alb").await;
    let group = create_http_target_group(&router, "rule-values-tg").await;
    let listener = create_listener(&router, &load_balancer, &group, 80).await;

    for row in condition_table() {
        let fields = [row.fields.clone(), fixed_response("404")].concat();
        let created = create_rule(&router, &listener, &row.priority.to_string(), &fields).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected 200, got {} {}",
            row.case, created.status, created.body
        );
        let arn = first_text(&created.body, "RuleArn")
            .unwrap_or_else(|| panic!("case {}: no RuleArn in {}", row.case, created.body));
        let read = describe_rule(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {}: reading the rule back expected 200, got {} {}",
            row.case, read.status, read.body
        );

        // The create's own answer and a later read of the stored rule answer the same
        // condition: the spellings are a property of the rule, not of one reply.
        for (source, reply) in [("the create's answer", &created), ("a later read", &read)] {
            let conditions = members(&reply.body, "Conditions");
            assert_eq!(
                conditions.len(),
                1,
                "case {} ({source}): expected the one condition that was created, got {}",
                row.case,
                reply.body
            );
            let condition = &conditions[0];
            assert_eq!(
                first_text(condition, "Field").as_deref(),
                Some(row.field),
                "case {} ({source}): expected Field {}, got {condition}",
                row.case,
                row.field
            );
            let top_level = outside_configs(condition);
            check_top_level(row.case, source, "Values", &top_level, row.values);
            check_top_level(row.case, source, "RegexValues", &top_level, row.regex_values);

            let config = element(condition, row.config).unwrap_or_else(|| {
                panic!(
                    "case {} ({source}): expected a {} in {condition}",
                    row.case, row.config
                )
            });
            for value in row.config_values {
                assert!(
                    config.contains(value),
                    "case {} ({source}): the {} must still report {value}, got {config}",
                    row.case,
                    row.config
                );
            }
        }
    }
}
