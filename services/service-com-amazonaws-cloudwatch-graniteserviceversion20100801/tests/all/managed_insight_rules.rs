// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the managed Contributor Insights rule loop: `PutManagedInsightRules`
//! enabling a managed rule for an Amazon Web Services resource, and
//! `ListManagedInsightRules` reporting it back for THAT resource.
//!
//! The rule under test: **a managed Contributor Insights rule belongs to the (template,
//! resource) pair it was enabled for** — so `ListManagedInsightRules` reports one description
//! per managed rule the account holds for the resource it is asked about, carrying that rule's
//! template, the resource's own ARN and the rule's name and state; enabling the same template
//! for the same resource again names the same rule rather than a second one; and a resource
//! the rule was not enabled for reports none of it.
//!
//! Every table row is a distinct input along what the rule generalises over (the template
//! name's length and the characters its pattern allows, the form of the resource ARN — its
//! partition, whether it carries a Region and an account, and how its resource field names the
//! resource —, whether the request carries tags, how many templates one resource has enabled,
//! and how many rules a page holds), so the happy path is one row among many rather than the
//! specification.
//!
//! Each row publishes the template it enables into the account's managed-rule catalog first: a
//! caller names a template the service publishes ("as returned by `ListManagedInsightRules`",
//! `ManagedRule$TemplateName`), so what a caller may enable is set up through the store, the way
//! the service sets it up. Which templates may be named, and what the operation answers for one
//! it cannot find, is `managed_insight_rule_catalog.rs`.

use crate::harness::{
    build_router, build_router_with_engine, call, error_code, error_message, ok,
    publish_managed_rule_template,
};
use serde_json::{json, Value};

/// The `Schema` a managed rule carries: the schema "For managed rules, this is `{"Name":
/// "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`) names, as the compact string the
/// member carries it as.
const MANAGED_RULE_SCHEMA: &str = "ServiceLogRule/1";

/// How this service words a `ResourceARN` that is not an ARN.
const INVALID_ARN_MESSAGE: &str = "INVALID_RESOURCE_ARN: Invalid ARN.";

/// One `PutManagedInsightRules` entry.
fn managed_rule(template: &str, resource_arn: &str, tags: Option<Value>) -> Value {
    let mut rule = json!({"TemplateName": template, "ResourceARN": resource_arn});
    if let Some(tags) = tags {
        rule["Tags"] = tags;
    }
    rule
}

/// The descriptions `ListManagedInsightRules` reported, in the order it reported them.
fn descriptions(body: &Value) -> Vec<Value> {
    body.get("ManagedRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One member of a reported description, as a string.
fn member(description: &Value, name: &str) -> String {
    description
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The rule `DescribeInsightRules` reported under `name`, if any.
async fn described_rule(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Option<Value> {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")?
        .as_array()?
        .iter()
        .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// The resource ARN forms a row may enable a managed rule for. Each is a well-formed Amazon
/// Resource Name: six fields, a non-empty partition, vendor and resource, and a Region and
/// account that may be empty for a partition-global resource
/// (`services/common/src/arn.rs`).
const RESOURCES: [(&str, &str); 5] = [
    (
        "a DynamoDB table",
        "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
    ),
    (
        "a DynamoDB table index, whose resource field carries several segments",
        "arn:aws:dynamodb:us-east-1:123456789012:table/Orders/index/by-customer",
    ),
    (
        "a Lambda function in another partition",
        "arn:aws-cn:lambda:cn-north-1:123456789012:function:checkout",
    ),
    (
        "an S3 bucket, which carries neither Region nor account",
        "arn:aws:s3:::forged-access-logs",
    ),
    (
        "a resource whose field is a bare id",
        "arn:aws:kinesis:eu-west-1:123456789012:stream-forged",
    ),
];

#[tokio::test]
async fn every_enabled_managed_rule_is_reported_for_its_own_resource() {
    // Rows differ along the template name's length and the characters its pattern allows
    // (`^[0-9A-Za-z][\-\.\_0-9A-Za-z]{0,126}[0-9A-Za-z]$`), the form of the resource ARN, and
    // whether the request carries tags.
    let long_template = format!("T{}1", "a".repeat(126));
    let templates: Vec<(&str, String, Option<Value>)> = vec![
        ("a two-character template name", "R1".to_string(), None),
        (
            "a template name carrying every punctuation the pattern allows",
            "Service-Log.Rule_0".to_string(),
            None,
        ),
        (
            "a 128-character template name",
            long_template.clone(),
            None,
        ),
        (
            "a template enabled with tags",
            "TaggedRule".to_string(),
            Some(json!([{"Key": "team", "Value": "observability"}])),
        ),
    ];

    for (resource_case, resource_arn) in RESOURCES {
        for (template_case, template, tags) in &templates {
            let case = format!("{resource_case} / {template_case}");
            // A fresh emulator per row, so the row's own call is the only thing that could
            // have enabled a rule, with the row's template published for the row's resource:
            // a caller may only enable a template the service publishes
            // (`ManagedRule$TemplateName`).
            let (router, engine) = build_router_with_engine();
            publish_managed_rule_template(&engine, template, resource_arn);

            let put = ok(
                &router,
                "PutManagedInsightRules",
                &json!({"ManagedRules": [managed_rule(template, resource_arn, tags.clone())]}),
            )
            .await;
            assert_eq!(
                put.get("Failures").and_then(Value::as_array).map(Vec::len),
                Some(0),
                "case {case:?}: enabling one managed rule must report no failure, got {put}"
            );

            let listed = ok(
                &router,
                "ListManagedInsightRules",
                &json!({"ResourceARN": resource_arn}),
            )
            .await;
            let reported = descriptions(&listed);
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected the one enabled managed rule, got {listed}"
            );
            let description = &reported[0];
            assert_eq!(
                member(description, "TemplateName"),
                *template,
                "case {case:?}: expected the template the rule was enabled from, got \
                 {description}"
            );
            assert_eq!(
                member(description, "ResourceARN"),
                resource_arn,
                "case {case:?}: expected the resource the rule was enabled for, got \
                 {description}"
            );
            let rule_state = description.get("RuleState").cloned().unwrap_or(Value::Null);
            assert_eq!(
                member(&rule_state, "State"),
                "ENABLED",
                "case {case:?}: an enabled managed rule must be reported ENABLED, got \
                 {description}"
            );
            let rule_name = member(&rule_state, "RuleName");
            assert!(
                !rule_name.is_empty() && rule_name.len() <= 128,
                "case {case:?}: expected a rule name inside the modelled 1..128 length, got \
                 {rule_name:?}"
            );

            // The managed rule is one of this account's Contributor Insights rules, so the
            // rule the description names reads back through `DescribeInsightRules`.
            let described = described_rule(&router, &rule_name).await;
            let described = described.unwrap_or_else(|| {
                panic!("case {case:?}: the managed rule {rule_name:?} must be described")
            });
            assert_eq!(
                described.get("ManagedRule").and_then(Value::as_bool),
                Some(true),
                "case {case:?}: a managed rule must be described as managed, got {described}"
            );
            assert_eq!(
                member(&described, "Schema"),
                MANAGED_RULE_SCHEMA,
                "case {case:?}: expected the managed-rule schema, got {described}"
            );
            assert_eq!(
                member(&described, "State"),
                "ENABLED",
                "case {case:?}: expected the described rule enabled, got {described}"
            );

            // Another resource holds no RULE of it: the row's template is published for the
            // vendor and resource type of the row's resource, so it is available for the other
            // DynamoDB table exactly when the row's resource is a DynamoDB table too — and
            // there its state names no rule, because nothing enabled it.
            let other_arn = "arn:aws:dynamodb:us-east-1:123456789012:table/Other";
            let other = ok(
                &router,
                "ListManagedInsightRules",
                &json!({"ResourceARN": other_arn}),
            )
            .await;
            let available_for_other: Vec<Value> = if resource_arn.starts_with("arn:aws:dynamodb:")
                && resource_arn.contains(":table/")
            {
                vec![json!({
                    "TemplateName": template,
                    "ResourceARN": other_arn,
                    "RuleState": {},
                })]
            } else {
                Vec::new()
            };
            assert_eq!(
                descriptions(&other),
                available_for_other,
                "case {case:?}: a resource no rule was enabled for must report only the \
                 templates available for it, each naming no rule in its state, got {other}"
            );
            let again = ok(
                &router,
                "ListManagedInsightRules",
                &json!({"ResourceARN": resource_arn}),
            )
            .await;
            assert_eq!(
                descriptions(&again),
                reported,
                "case {case:?}: a second listing must report the same rules, got {again}"
            );
        }
    }
}

#[tokio::test]
async fn several_templates_on_one_resource_are_reported_by_template_name() {
    let resource = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    // Rows differ along how many templates one request enables and in which order it names
    // them; the reported order is the templates' own, whatever the request's was.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one template", vec!["Rule-A"]),
        (
            "three templates named out of order",
            vec!["Rule-C", "Rule-A", "Rule-B"],
        ),
        (
            "templates whose names differ only in case and punctuation",
            vec!["rule.a", "Rule-A", "Rule_A", "RuleA"],
        ),
    ];

    for (case, templates) in &cases {
        let (router, engine) = build_router_with_engine();
        for template in templates {
            publish_managed_rule_template(&engine, template, resource);
        }
        let rules: Vec<Value> = templates
            .iter()
            .map(|template| managed_rule(template, resource, None))
            .collect();
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": rules}),
        )
        .await;
        assert_eq!(
            put.get("Failures").and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {case:?}: every distinct template must be enabled, got {put}"
        );

        let listed = ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource}),
        )
        .await;
        let reported: Vec<String> = descriptions(&listed)
            .iter()
            .map(|description| member(description, "TemplateName"))
            .collect();
        let mut expected: Vec<String> = templates.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected every enabled template, ordered by name, got {listed}"
        );
    }
}

#[tokio::test]
async fn enabling_the_same_template_again_names_the_same_rule() {
    let resource = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "Rule-A", resource);

    let first = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [managed_rule("Rule-A", resource, None)]}),
    )
    .await;
    assert_eq!(
        first.get("Failures").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "the first call must enable the rule, got {first}"
    );
    let before = descriptions(
        &ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource}),
        )
        .await,
    );

    // "If a previously created managed rule is currently disabled, a subsequent call to this
    // API will re-enable it." (`PutManagedInsightRules`) — so a second call names the rule the
    // first one created rather than a second rule, and the tags it carries are ignored.
    let second = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [
            managed_rule("Rule-A", resource, Some(json!([{"Key": "team", "Value": "late"}]))),
        ]}),
    )
    .await;
    assert_eq!(
        second.get("Failures").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "re-enabling must report no failure, got {second}"
    );
    let after = descriptions(
        &ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource}),
        )
        .await,
    );
    assert_eq!(
        after, before,
        "enabling the same template twice must report the same one rule, got {after:?}"
    );
}

#[tokio::test]
async fn a_page_holds_at_most_max_results_rules_and_the_token_continues_it() {
    let resource = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    // Rows differ along how many rules the resource holds and how many one page may hold,
    // including `MaxResults`'s published bounds (range 1..500) and a page larger than the
    // whole set.
    let cases: Vec<(&str, usize, Option<i64>)> = vec![
        ("every rule on the default page", 7, None),
        ("one rule per page, the published minimum", 5, Some(1)),
        ("a page that holds two of five", 5, Some(2)),
        ("a page larger than the whole set", 3, Some(500)),
        ("a page exactly as large as the set", 4, Some(4)),
    ];

    for (case, rule_count, max_results) in &cases {
        let (router, engine) = build_router_with_engine();
        for index in 0..*rule_count {
            publish_managed_rule_template(&engine, &format!("Rule-{index:02}"), resource);
        }
        let rules: Vec<Value> = (0..*rule_count)
            .map(|index| managed_rule(&format!("Rule-{index:02}"), resource, None))
            .collect();
        ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": rules}),
        )
        .await;

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"ResourceARN": resource});
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let listed = ok(&router, "ListManagedInsightRules", &request).await;
            let page = descriptions(&listed);
            if let Some(max_results) = max_results {
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= *max_results,
                    "case {case:?}: a page must hold at most MaxResults rules, got {listed}"
                );
            }
            seen.extend(
                page.iter()
                    .map(|description| member(description, "TemplateName")),
            );
            next_token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= rule_count + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {rule_count} rules"
            );
            if next_token.is_none() {
                break;
            }
        }

        let expected: Vec<String> = (0..*rule_count)
            .map(|index| format!("Rule-{index:02}"))
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages together must report every rule exactly once"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let resource = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "Rule-A", resource);
    ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [managed_rule("Rule-A", resource, None)]}),
    )
    .await;

    // Rows differ along how the token is malformed: not the encoding this service mints, an
    // odd number of digits, and a token that decodes to bytes that are not text.
    for case in ["not-a-token", "abc", "ff"] {
        let (status, body) = call(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource, "NextToken": case}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNextToken"),
            "case {case:?}: expected HTTP 400 InvalidNextToken, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_reported_about_that_member() {
    // Rows differ along which part of the ARN form is broken: the literal first field, a
    // missing field, and an empty partition, vendor or resource — the fields an ARN cannot
    // omit (`services/common/src/arn.rs`).
    let cases: Vec<(&str, &str)> = vec![
        ("a bare name", "Orders"),
        (
            "a slash-separated resource path",
            "dynamodb/table/Orders",
        ),
        ("too few fields", "arn:aws:dynamodb:us-east-1:123456789012"),
        (
            "an empty partition",
            "arn::dynamodb:us-east-1:123456789012:table/Orders",
        ),
        (
            "an empty vendor",
            "arn:aws::us-east-1:123456789012:table/Orders",
        ),
        (
            "an empty resource",
            "arn:aws:dynamodb:us-east-1:123456789012:",
        ),
    ];

    for (case, resource_arn) in &cases {
        let router = build_router();

        // `PutManagedInsightRules` is a partial-failure batch: the entry is reported back on
        // the HTTP 200 rather than failing the whole call, so the other entries of the same
        // request are still enabled. "If the operation could not process one of the rules, the
        // following data is returned for each of those rules." (`PartialFailure`)
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule("Rule-A", resource_arn, None)]}),
        )
        .await;
        assert_eq!(
            put.get("Failures").and_then(Value::as_array).cloned(),
            Some(vec![json!({
                "FailureResource": "Rule-A",
                "ExceptionType": "InvalidParameterException",
                "FailureCode": "TEMPLATE_NAME",
                "FailureDescription": "Invalid ARN.",
            })]),
            "case {case:?}: expected the entry reported back about a value that is not an ARN, \
             got {put}"
        );
        // A reported entry enables nothing: no Contributor Insights rule exists at all.
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            described
                .get("InsightRules")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: a reported entry must enable no rule, got {described}"
        );

        // The READ takes the ARN as its own required parameter rather than as one entry of a
        // batch, so a value that is not an ARN is the whole request's fault.
        let (status, body) = call(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource_arn}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected ListManagedInsightRules to refuse the ARN with HTTP 400 \
             InvalidParameterValueException, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            INVALID_ARN_MESSAGE,
            "case {case:?}: expected the service's wording for an invalid ARN, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_managed_rule_is_refused() {
    let router = build_router();
    let (status, body) = call(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": []}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "expected HTTP 400 InvalidParameterValueException for a request naming no rule to \
         enable, got {status} with {body}"
    );
    assert_eq!(
        error_message(&body),
        "EMPTY_MANAGED_RULES: ManagedRules cannot be an empty array.",
        "expected the service's wording for an empty ManagedRules, got {body}"
    );
    let described = ok(&router, "DescribeInsightRules", &json!({})).await;
    assert_eq!(
        described
            .get("InsightRules")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused request must enable no rule, got {described}"
    );
}

#[tokio::test]
async fn more_tags_than_a_rule_may_carry_are_refused() {
    let resource = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";
    // "You can associate as many as 50 tags with a rule." (`ManagedRule$Tags`) — the bound
    // itself is accepted and one past it is refused.
    let tags = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
            .collect()
    };

    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "Rule-A", resource);
    publish_managed_rule_template(&engine, "Rule-B", resource);
    let put = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [managed_rule("Rule-A", resource, Some(tags(50)))]}),
    )
    .await;
    assert_eq!(
        put.get("Failures").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "case 50 tags: the published bound must be accepted, got {put}"
    );

    let (status, body) = call(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [managed_rule("Rule-B", resource, Some(tags(51)))]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "case 51 tags: expected HTTP 400 InvalidParameterValueException, got {status} with {body}"
    );
    let listed = ok(
        &router,
        "ListManagedInsightRules",
        &json!({"ResourceARN": resource}),
    )
    .await;
    let reported: Vec<(String, bool)> = descriptions(&listed)
        .iter()
        .map(|description| {
            (
                member(description, "TemplateName"),
                description.pointer("/RuleState/RuleName").is_some(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![("Rule-A".to_string(), true), ("Rule-B".to_string(), false)],
        "case 51 tags: the refused request must have enabled nothing, leaving the published \
         Rule-B merely available, got {listed}"
    );
}
