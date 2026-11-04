// SPDX-License-Identifier: CC0-1.0

//! Cover for the catalog side of the managed Contributor Insights rules: WHICH template a
//! `PutManagedInsightRules` entry may name, and what the operation answers for one it cannot
//! resolve.
//!
//! The rule under test: **a `ManagedRule` entry's `TemplateName` names a template the catalog
//! publishes for that entry's resource** — "The template name for the managed Contributor
//! Insights rule, as returned by `ListManagedInsightRules`." (`ManagedRule$TemplateName`) — so
//! an entry the catalog cannot resolve a managed rule for is one the operation could not
//! process: it is reported back on the HTTP 200 as exactly one `PartialFailure` about the
//! template name and enables nothing ("If the operation could not process one of the rules, the
//! following data is returned for each of those rules.", `PartialFailure`), while the entries of
//! the same request that DO name a published template are enabled. Which CLASS the entry carries
//! follows the stage the resolution stopped at: a template name no published template answers to
//! is not found, a resource whose vendor publishes no managed rule at all is the service's own
//! failure, and a resource of a published vendor that no template applies to is reported about
//! the ARN.
//!
//! Rows differ along what the rule generalises over: how the name misses the catalog (nothing
//! published at all, a published name in another case, a published template of another vendor or
//! another resource type), the length and punctuation of the template name
//! (`^[0-9A-Za-z][\-\.\_0-9A-Za-z]{0,126}[0-9A-Za-z]$`, 1..128), the form of the resource ARN,
//! and how many entries one request carries.

use crate::harness::{build_router_with_engine, ok, publish_managed_rule_template};
use serde_json::{json, Value};

/// A resource ARN of each form a managed rule may be enabled for: six fields, a non-empty
/// partition, vendor and resource, and a Region and account that may be empty for a
/// partition-global resource (`services/common/src/arn.rs`).
const TABLE: &str = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
/// A CloudWatch alarm, the second resource type the recorded behaviour measures.
const ALARM: &str = "arn:aws:cloudwatch:us-east-1:123456789012:alarm:orders-latency";
/// An S3 bucket, which carries neither Region nor account.
const BUCKET: &str = "arn:aws:s3:::forged-access-logs";

/// One `PutManagedInsightRules` entry.
fn managed_rule(template: &str, resource_arn: &str) -> Value {
    json!({"TemplateName": template, "ResourceARN": resource_arn})
}

/// The `Failures` of an answer, in the order it reported them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The entry CloudWatch reports for an entry whose template NAME no published template answers
/// to: the submitted template name as the `FailureResource`, and fixed text for the rest of it.
fn template_not_found(template_name: &str) -> Value {
    json!({
        "FailureResource": template_name,
        "ExceptionType": "ResourceNotFoundException",
        "FailureCode": "TEMPLATE_NAME",
        "FailureDescription": "Could not find template with the specified templateName.",
    })
}

/// The entry reported for an entry whose resource's VENDOR the catalog publishes no managed
/// rule for at all: the same sentence, under the receiver-side class.
fn no_managed_rules_for_the_vendor(template_name: &str) -> Value {
    json!({
        "FailureResource": template_name,
        "ExceptionType": "InternalFailure",
        "FailureCode": "TEMPLATE_NAME",
        "FailureDescription": "Could not find template with the specified templateName.",
    })
}

/// The entry reported for an entry whose resource is of a published VENDOR but of a kind no
/// published template applies to: the fault is then the resource the ARN names.
fn invalid_resource_arn(template_name: &str) -> Value {
    json!({
        "FailureResource": template_name,
        "ExceptionType": "InvalidParameterException",
        "FailureCode": "TEMPLATE_NAME",
        "FailureDescription": "Invalid ResourceArn.",
    })
}

/// Whether a reported description says a rule is enabled from its template: the state's members
/// are the RULE's own, so a template nothing has enabled reports a state naming none.
fn names_a_rule(description: &Value) -> bool {
    description.pointer("/RuleState/RuleName").is_some()
}

/// The descriptions `ListManagedInsightRules` reports for `resource_arn`.
async fn listed(
    router: &smithy_server_runtime::MultiServiceRouter,
    resource_arn: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "ListManagedInsightRules",
        &json!({"ResourceARN": resource_arn}),
    )
    .await;
    body.get("ManagedRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The names of the Contributor Insights rules the account holds at all.
async fn rule_names(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            rule.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row of the unresolvable-template table: what the catalog publishes, the entry the
/// request carries, and the entry the operation reports back for it.
struct Row {
    /// What the row's assertions name it by.
    case: &'static str,
    /// The template the catalog publishes and the resource it is published for, when the row
    /// publishes one at all.
    published: Option<(&'static str, &'static str)>,
    /// The template name the request's entry carries.
    template: String,
    /// The resource the request's entry carries.
    resource_arn: &'static str,
    /// Which `Failures` entry the operation reports for it, built from the template name.
    reported: fn(&str) -> Value,
}

#[tokio::test]
async fn an_entry_whose_template_the_catalog_does_not_publish_is_reported_back() {
    let long_name = format!("T{}1", "a".repeat(126));
    let rows: Vec<Row> = vec![
        Row {
            case: "nothing published, a name no template answers to",
            published: None,
            template: "no-such-template-xyz".to_string(),
            resource_arn: TABLE,
            reported: template_not_found,
        },
        Row {
            case: "nothing published, the shortest name the pattern allows",
            published: None,
            template: "T1".to_string(),
            resource_arn: ALARM,
            reported: template_not_found,
        },
        Row {
            case: "nothing published, a 128-character name",
            published: None,
            template: long_name,
            resource_arn: BUCKET,
            reported: template_not_found,
        },
        Row {
            case: "nothing published, a name carrying every punctuation the pattern allows",
            published: None,
            template: "Service-Log.Rule_0".to_string(),
            resource_arn: TABLE,
            reported: template_not_found,
        },
        Row {
            case: "a template published under another case",
            published: Some(("Orders-Insights", TABLE)),
            template: "orders-insights".to_string(),
            resource_arn: TABLE,
            reported: template_not_found,
        },
        Row {
            case: "a template published for another vendor's resources",
            published: Some(("Orders-Insights", TABLE)),
            template: "Orders-Insights".to_string(),
            resource_arn: "arn:aws:kinesis:us-east-1:123456789012:stream/Orders",
            reported: no_managed_rules_for_the_vendor,
        },
        Row {
            case: "a template published for another resource type of the same vendor",
            published: Some(("Orders-Insights", TABLE)),
            template: "Orders-Insights".to_string(),
            resource_arn: "arn:aws:dynamodb:us-east-1:123456789012:backup/Orders",
            reported: invalid_resource_arn,
        },
        Row {
            case: "a template published for another vendor, named for this one's resource",
            published: Some(("Orders-Insights", ALARM)),
            template: "Orders-Insights".to_string(),
            resource_arn: TABLE,
            reported: no_managed_rules_for_the_vendor,
        },
    ];

    for row in &rows {
        let case = row.case;
        let template = row.template.as_str();
        let resource_arn = row.resource_arn;
        let (router, engine) = build_router_with_engine();
        if let Some((published_template, published_for)) = row.published {
            publish_managed_rule_template(&engine, published_template, published_for);
        }

        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        assert_eq!(
            failures(&put),
            vec![(row.reported)(template)],
            "case {case:?}: expected exactly one TEMPLATE_NAME failure for the entry the \
             catalog could not resolve, got {put}"
        );

        // Nothing is enabled: no Contributor Insights rule exists, and the resource reports no
        // managed rule naming one.
        let held = rule_names(&router).await;
        assert!(
            held.is_empty(),
            "case {case:?}: an entry that could not be resolved must enable no rule, got {held:?}"
        );
        let reported = listed(&router, resource_arn).await;
        let with_rule: Vec<&Value> = reported.iter().filter(|d| names_a_rule(d)).collect();
        assert!(
            with_rule.is_empty(),
            "case {case:?}: no managed rule may be reported as enabled, got {with_rule:?}"
        );
    }
}

#[tokio::test]
async fn an_entry_naming_a_published_template_is_enabled_and_read_back() {
    // Rows differ along the template name's length and punctuation and the form of the resource
    // ARN the template is published for.
    let long_name = format!("T{}1", "a".repeat(126));
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a DynamoDB table", "Orders-Insights", TABLE),
        ("a CloudWatch alarm", "T1", ALARM),
        (
            "an S3 bucket, which carries neither Region nor account",
            "Service-Log.Rule_0",
            BUCKET,
        ),
        (
            "a Lambda function in another partition",
            long_name.as_str(),
            "arn:aws-cn:lambda:cn-north-1:123456789012:function:checkout",
        ),
    ];

    for (case, template, resource_arn) in &cases {
        let (router, engine) = build_router_with_engine();
        publish_managed_rule_template(&engine, template, resource_arn);

        // Before anything enables it, the published template is reported as available for the
        // resource and carries no rule state — the catalog `PutManagedInsightRules` resolves
        // against is the one this operation reports from.
        let available = listed(&router, resource_arn).await;
        assert_eq!(
            available
                .iter()
                .map(|description| description
                    .get("TemplateName")
                    .and_then(Value::as_str)
                    .unwrap_or_default())
                .collect::<Vec<&str>>(),
            vec![*template],
            "case {case:?}: expected the published template to be available, got {available:?}"
        );
        assert!(
            available.first().is_some_and(|description| description
                .pointer("/RuleState/RuleName")
                .is_none()),
            "case {case:?}: a template nothing has enabled names no rule in its state, got \
             {available:?}"
        );

        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        assert_eq!(
            failures(&put),
            Vec::<Value>::new(),
            "case {case:?}: a published template must be enabled with no failure, got {put}"
        );

        let reported = listed(&router, resource_arn).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one managed rule, got {reported:?}"
        );
        let description = &reported[0];
        assert_eq!(
            (
                description.get("TemplateName").and_then(Value::as_str),
                description.get("ResourceARN").and_then(Value::as_str),
                description
                    .pointer("/RuleState/State")
                    .and_then(Value::as_str),
            ),
            (Some(*template), Some(*resource_arn), Some("ENABLED")),
            "case {case:?}: expected the enabled rule's template, resource and state, got \
             {description}"
        );

        // The managed rule is one of this account's Contributor Insights rules, so it reads back
        // through `DescribeInsightRules` under the name the description names.
        let name = description
            .pointer("/RuleState/RuleName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let held = rule_names(&router).await;
        assert_eq!(
            held,
            vec![name.clone()],
            "case {case:?}: expected the enabled managed rule {name:?} to be described, got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn each_entry_of_one_request_is_processed_on_its_own() {
    // One request carrying published and unpublished templates, interleaved: the ones the
    // catalog answers are enabled and the others are reported back, in the order the request
    // named them.
    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "Orders-Insights", TABLE);
    publish_managed_rule_template(&engine, "Latency-Insights", ALARM);

    let put = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [
            managed_rule("missing-first", TABLE),
            managed_rule("Orders-Insights", TABLE),
            managed_rule("Latency-Insights", TABLE),
            managed_rule("Latency-Insights", ALARM),
            managed_rule("missing-last", ALARM),
        ]}),
    )
    .await;
    assert_eq!(
        failures(&put),
        vec![
            template_not_found("missing-first"),
            // Published for alarms, so for a table it is a template published for other
            // resources than the one the entry names.
            template_not_found("Latency-Insights"),
            template_not_found("missing-last"),
        ],
        "expected one failure per entry the catalog could not answer, in request order, got {put}"
    );

    let table_rules: Vec<(String, Option<String>)> = listed(&router, TABLE)
        .await
        .iter()
        .map(|description| {
            (
                description
                    .get("TemplateName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description
                    .pointer("/RuleState/State")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            )
        })
        .collect();
    assert_eq!(
        table_rules,
        vec![(
            "Orders-Insights".to_string(),
            Some("ENABLED".to_string())
        )],
        "expected only the published template enabled for the table, got {table_rules:?}"
    );

    let alarm_rules: Vec<String> = listed(&router, ALARM)
        .await
        .iter()
        .filter(|description| {
            description
                .pointer("/RuleState/State")
                .and_then(Value::as_str)
                == Some("ENABLED")
        })
        .map(|description| {
            description
                .get("TemplateName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        alarm_rules,
        vec!["Latency-Insights".to_string()],
        "expected the alarm's published template enabled, got {alarm_rules:?}"
    );

    // Two of the five entries were enabled, so the account holds exactly two rules.
    let held = rule_names(&router).await;
    assert_eq!(
        held.len(),
        2,
        "expected one rule per enabled entry and none for the rest, got {held:?}"
    );
}
