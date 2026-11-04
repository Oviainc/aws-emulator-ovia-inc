// SPDX-License-Identifier: CC0-1.0

//! Cover for the managed Contributor Insights rules the SERVICE itself publishes: the catalog
//! a resource's managed rules come from, the rule enabling one creates, and the per-entry
//! report of an entry the catalog could not resolve.
//!
//! The rules under test:
//!
//! 1. **The managed rules available for a resource are the templates the catalog publishes for
//!    that KIND of resource, whatever has been enabled** — "Returns a list that contains the
//!    number of managed Contributor Insights rules in your account."
//!    (`ListManagedInsightRules`), whose `ManagedRules` are "the managed rules that are
//!    available for the specified Amazon Web Services resource"
//!    (`ListManagedInsightRulesOutput$ManagedRules`). So a listing reports one description per
//!    published template, each carrying the resource the request named and a `RuleState` whose
//!    members — the rule's own name and state
//!    (`ManagedRuleState$RuleName`, `ManagedRuleState$State`) — are there only for a template a
//!    rule HAS been enabled from: enabling a rule and deleting it again change what the states
//!    say and never which templates are listed.
//! 2. **`PutManagedInsightRules` resolves every entry on its own and reports one `Failures`
//!    entry per entry it could not resolve, on the HTTP 200** — "If the operation could not
//!    process one of the rules, the following data is returned for each of those rules."
//!    (`PartialFailure`) — classed by the stage the resolution stopped at: a value that is not
//!    an ARN, an ARN that names no resource AWS issued one for, a resource whose service
//!    publishes no managed rule at all, a resource of a published service that no template
//!    applies to, and a template name no published template answers to.
//! 3. **A managed rule's NAME is derived from the template and the resource** — "If a
//!    previously created managed rule is currently disabled, a subsequent call to this API will
//!    re-enable it." (`PutManagedInsightRules`) is only possible for a name the pair decides:
//!    the template without its trailing version, the resource's own id, and that version.
//!
//! Every table row is a distinct input along what the rules generalise over: which published
//! template is enabled, the Region, account, partition and resource id of the VPC endpoint
//! service ARN, which stage of the resolution an entry stops at, and how many descriptions a
//! page holds.

use crate::harness::{
    build_router, build_router_with_engine, call, call_in_query_mode, error_code, error_message,
    ok, publish_managed_rule_template,
};
use serde_json::{json, Value};

/// The managed-rule templates the service publishes for a VPC endpoint service, as
/// `ListManagedInsightRules` reports them for one: the rules enabled for the resource first,
/// then the templates nothing has enabled in template-name order
/// (`crate::state::insight_rule_records`'s catalog).
const PUBLISHED_TEMPLATES: [&str; 4] = [
    "VpcEndpointService-ActiveConnectionsByEndpointId-v1",
    "VpcEndpointService-BytesByEndpointId-v1",
    "VpcEndpointService-NewConnectionsByEndpointId-v1",
    "VpcEndpointService-RstPacketsByEndpointId-v1",
];

/// One `PutManagedInsightRules` entry.
fn managed_rule(template: &str, resource_arn: &str) -> Value {
    json!({"TemplateName": template, "ResourceARN": resource_arn})
}

/// The descriptions `ListManagedInsightRules` reports for `resource_arn`, in the order it
/// reports them.
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

/// The template names a listing reported, in order.
fn template_names(descriptions: &[Value]) -> Vec<String> {
    descriptions
        .iter()
        .map(|description| {
            description
                .get("TemplateName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `Failures` of an answer, in the order it reported them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The VPC endpoint service ARNs a row may enable a managed rule for: the resource kind the
/// catalog publishes its templates for, in each ARN form AWS issues one in — different
/// Regions, accounts and partitions, and both id lengths a VPC endpoint service carries.
const ENDPOINT_SERVICES: [(&str, &str); 4] = [
    (
        "a VPC endpoint service with a long id",
        "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0123456789abcdef0",
    ),
    (
        "one in another Region and account",
        "arn:aws:ec2:eu-west-3:210987654321:vpc-endpoint-service/vpce-svc-0abc1234",
    ),
    (
        "one in the China partition",
        "arn:aws-cn:ec2:cn-north-1:123456789012:vpc-endpoint-service/vpce-svc-00ff11ee22dd33cc4",
    ),
    (
        "one in the GovCloud partition",
        "arn:aws-us-gov:ec2:us-gov-west-1:123456789012:vpc-endpoint-service/vpce-svc-0f0f0f0f",
    ),
];

#[tokio::test]
async fn every_published_template_is_reported_for_a_vpc_endpoint_service() {
    // Rows differ along the ARN form of the resource and along WHICH of the published
    // templates the rule is enabled from — every one of them, not only the first.
    for (resource_case, resource_arn) in ENDPOINT_SERVICES {
        for enabled in PUBLISHED_TEMPLATES {
            let case = format!("{resource_case} / {enabled}");
            let router = build_router();

            // Before anything is enabled the whole catalog is already available for the
            // resource, each description naming the resource and no rule.
            let available = listed(&router, resource_arn).await;
            assert_eq!(
                template_names(&available),
                PUBLISHED_TEMPLATES.to_vec(),
                "case {case:?}: expected every published template to be available, got \
                 {available:?}"
            );
            for description in &available {
                assert_eq!(
                    (
                        description.get("ResourceARN").and_then(Value::as_str),
                        description.pointer("/RuleState/RuleName"),
                    ),
                    (Some(resource_arn), None),
                    "case {case:?}: an available template names the resource and no rule, got \
                     {description}"
                );
            }

            let put = ok(
                &router,
                "PutManagedInsightRules",
                &json!({"ManagedRules": [managed_rule(enabled, resource_arn)]}),
            )
            .await;
            assert_eq!(
                failures(&put),
                Vec::<Value>::new(),
                "case {case:?}: enabling a published template must report no failure, got {put}"
            );

            // The enabled template is reported FIRST, with the rule's name and state; the rest
            // of the catalog is still listed, each naming no rule.
            let reported = listed(&router, resource_arn).await;
            let mut expected: Vec<String> = vec![enabled.to_string()];
            expected.extend(
                PUBLISHED_TEMPLATES
                    .iter()
                    .filter(|template| **template != enabled)
                    .map(ToString::to_string),
            );
            assert_eq!(
                template_names(&reported),
                expected,
                "case {case:?}: expected the enabled template first and the rest after it, got \
                 {reported:?}"
            );
            let enabled_description = &reported[0];
            assert_eq!(
                enabled_description
                    .pointer("/RuleState/State")
                    .and_then(Value::as_str),
                Some("ENABLED"),
                "case {case:?}: the enabled template's state must be ENABLED, got \
                 {enabled_description}"
            );
            let rule_name = enabled_description
                .pointer("/RuleState/RuleName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                rule_name,
                expected_rule_name(enabled, resource_arn),
                "case {case:?}: expected the name the template and the resource decide, got \
                 {enabled_description}"
            );
            for description in &reported[1..] {
                assert_eq!(
                    description.pointer("/RuleState/RuleName"),
                    None,
                    "case {case:?}: a template nothing enabled names no rule, got {description}"
                );
            }

            // The rule reads back as one of this account's Contributor Insights rules, and
            // deleting it leaves the catalog listed with no rule named in any state.
            let described = ok(&router, "DescribeInsightRules", &json!({})).await;
            let held: Vec<&str> = described
                .get("InsightRules")
                .and_then(Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter_map(|rule| rule.get("Name").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                held,
                vec![rule_name.as_str()],
                "case {case:?}: expected the enabled managed rule to be described, got \
                 {described}"
            );
            ok(
                &router,
                "DeleteInsightRules",
                &json!({"RuleNames": [&rule_name]}),
            )
            .await;
            let after_delete = listed(&router, resource_arn).await;
            assert_eq!(
                template_names(&after_delete),
                PUBLISHED_TEMPLATES.to_vec(),
                "case {case:?}: the catalog survives the rule's deletion, got {after_delete:?}"
            );
            for description in &after_delete {
                assert_eq!(
                    description.pointer("/RuleState/RuleName"),
                    None,
                    "case {case:?}: no rule is enabled any more, got {description}"
                );
            }
        }
    }
}

/// The name the managed rule enabled from `template` for `resource_arn` takes: the template
/// without its trailing version, the resource's own id, and that version.
fn expected_rule_name(template: &str, resource_arn: &str) -> String {
    let resource_id = resource_arn
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .to_string();
    match template.rsplit_once("-v") {
        Some((head, version)) if version.chars().all(|c| c.is_ascii_digit()) => {
            format!("{head}-{resource_id}-v{version}")
        }
        _ => format!("{template}-{resource_id}"),
    }
}

#[tokio::test]
async fn a_managed_rule_name_is_derived_from_the_template_and_the_resource() {
    // Rows differ along the form of the template name — a published one carrying a version, an
    // account-published one carrying none, and one at the 128-character bound of
    // `TemplateName` — and along how the resource's id sits in its ARN.
    let long_template = format!("T{}1", "a".repeat(126));
    let rows: Vec<(&str, String, &str, bool)> = vec![
        (
            "a published template, whose name carries a version",
            PUBLISHED_TEMPLATES[1].to_string(),
            ENDPOINT_SERVICES[0].1,
            false,
        ),
        (
            "an account-published template with no version",
            "Orders-Insights".to_string(),
            "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
            true,
        ),
        (
            "an account-published template for a resource whose field is a bare name",
            "Access-Logs".to_string(),
            "arn:aws:s3:::forged-access-logs",
            true,
        ),
        (
            "a 128-character template name",
            long_template,
            "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
            true,
        ),
    ];

    for (case, template, resource_arn, publish) in &rows {
        let (router, engine) = build_router_with_engine();
        if *publish {
            publish_managed_rule_template(&engine, template, resource_arn);
        }
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        assert_eq!(
            failures(&put),
            Vec::<Value>::new(),
            "case {case:?}: a published template must be enabled, got {put}"
        );

        let reported = listed(&router, resource_arn).await;
        let rule_name = reported
            .iter()
            .find(|description| {
                description.get("TemplateName").and_then(Value::as_str) == Some(template.as_str())
            })
            .and_then(|description| description.pointer("/RuleState/RuleName"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // `InsightRuleName` is modelled `length 1..128`, `@pattern "^[\x20-\x7E]+$"`, so a
        // derived name is held inside that length whatever the template's own length.
        assert!(
            (1..=128).contains(&rule_name.chars().count())
                && rule_name.chars().all(|c| c.is_ascii_graphic() || c == ' '),
            "case {case:?}: expected a name inside the modelled 1..128 length and pattern, got \
             {rule_name:?}"
        );
        let expected = expected_rule_name(template, resource_arn);
        let expected: String = expected.chars().take(128).collect();
        assert_eq!(
            rule_name, expected,
            "case {case:?}: expected the name the template and the resource decide, got \
             {rule_name:?}"
        );

        // Enabling the same template again names the SAME rule rather than a second one.
        ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        let again = listed(&router, resource_arn).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second call must report the same one rule, got {again:?}"
        );
    }
}

/// One row of the resolution table: the entry a request carries, and the class and sentence the
/// `Failures` entry reported for it.
struct Unresolvable {
    /// What the row's assertions name it by.
    case: &'static str,
    /// The template name the entry carries.
    template: &'static str,
    /// The resource the entry carries.
    resource_arn: &'static str,
    /// The `ExceptionType` of the reported entry.
    exception_type: &'static str,
    /// Its `FailureDescription`.
    description: &'static str,
}

/// What CloudWatch states for an entry it could not resolve a template for.
const TEMPLATE_NOT_FOUND: &str = "Could not find template with the specified templateName.";

#[tokio::test]
async fn each_stage_of_the_resolution_is_reported_as_its_own_fault() {
    // Rows differ along the stage the resolution stops at and, inside the stages about the
    // ARN, along which field of it is not one AWS issues.
    let rows: [Unresolvable; 9] = [
        Unresolvable {
            case: "a value that is not an ARN at all",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "vpce-svc-0123456789abcdef0",
            exception_type: "InvalidParameterException",
            description: "Invalid ARN.",
        },
        Unresolvable {
            case: "an ARN with too few fields",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:ec2:us-east-1:123456789012",
            exception_type: "InvalidParameterException",
            description: "Invalid ARN.",
        },
        Unresolvable {
            case: "an ARN naming the resource type and no resource of it",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/",
            exception_type: "InvalidParameterException",
            description: "Invalid ResourceArn.",
        },
        Unresolvable {
            case: "an ARN whose account is not an account id",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:ec2:us-east-1:12345:vpc-endpoint-service/vpce-svc-0abc1234",
            exception_type: "InvalidParameterException",
            description: "Invalid ResourceArn.",
        },
        Unresolvable {
            case: "an ARN whose Region is not a Region label",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:ec2:US EAST 1:123456789012:vpc-endpoint-service/vpce-svc-0a",
            exception_type: "InvalidParameterException",
            description: "Invalid ResourceArn.",
        },
        Unresolvable {
            case: "an ARN of no AWS partition",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:azure:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0a",
            exception_type: "InvalidParameterException",
            description: "Invalid ResourceArn.",
        },
        Unresolvable {
            case: "a resource of a service the catalog publishes no managed rule for",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
            exception_type: "InternalFailure",
            description: TEMPLATE_NOT_FOUND,
        },
        Unresolvable {
            case: "another resource type of a service the catalog does publish for",
            template: PUBLISHED_TEMPLATES[2],
            resource_arn: "arn:aws:ec2:us-east-1:123456789012:instance/i-0123456789abcdef0",
            exception_type: "InvalidParameterException",
            description: "Invalid ResourceArn.",
        },
        Unresolvable {
            case: "a template name no published template answers to",
            template: "NotARealTemplateName",
            resource_arn: ENDPOINT_SERVICES[0].1,
            exception_type: "ResourceNotFoundException",
            description: TEMPLATE_NOT_FOUND,
        },
    ];

    for row in &rows {
        let case = row.case;
        let router = build_router();
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(row.template, row.resource_arn)]}),
        )
        .await;
        assert_eq!(
            failures(&put),
            vec![json!({
                "FailureResource": row.template,
                "ExceptionType": row.exception_type,
                "FailureCode": "TEMPLATE_NAME",
                "FailureDescription": row.description,
            })],
            "case {case:?}: expected one entry reported about the stage that stopped the \
             resolution, got {put}"
        );
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            described
                .get("InsightRules")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: a reported entry must enable no rule, got {described}"
        );
    }
}

#[tokio::test]
async fn a_template_name_outside_the_modelled_pattern_is_reported_and_not_refused() {
    // `TemplateName` is modelled `@pattern "^[0-9A-Za-z][\-\.\_0-9A-Za-z]{0,126}[0-9A-Za-z]$"`,
    // and the operation does not refuse a request for it: the name is resolved against the
    // catalog like any other, and no published template answers to it. Rows differ along which
    // part of the pattern the name breaks.
    let resource_arn = ENDPOINT_SERVICES[0].1;
    for template in [
        "not a valid template name!!",
        "-leading-punctuation",
        "trailing-punctuation-",
        "contains/slash",
    ] {
        let router = build_router();
        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": [managed_rule(template, resource_arn)]}),
        )
        .await;
        assert_eq!(
            failures(&put),
            vec![json!({
                "FailureResource": template,
                "ExceptionType": "ResourceNotFoundException",
                "FailureCode": "TEMPLATE_NAME",
                "FailureDescription": TEMPLATE_NOT_FOUND,
            })],
            "case {template:?}: expected the entry reported back on a 200, got {put}"
        );
    }
}

#[tokio::test]
async fn one_request_is_resolved_entry_by_entry() {
    // A batch carrying, in order: an entry the catalog resolves, one whose ARN is not an ARN,
    // one of a service with no managed rules, and a second entry the catalog resolves. The two
    // it resolves are enabled and the other two are reported back, in request order.
    let endpoint_service = ENDPOINT_SERVICES[0].1;
    let router = build_router();
    let put = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [
            managed_rule(PUBLISHED_TEMPLATES[0], endpoint_service),
            managed_rule(PUBLISHED_TEMPLATES[1], "not-an-arn"),
            managed_rule(PUBLISHED_TEMPLATES[1], "arn:aws:sqs:us-east-1:123456789012:orders"),
            managed_rule(PUBLISHED_TEMPLATES[3], endpoint_service),
        ]}),
    )
    .await;
    assert_eq!(
        failures(&put),
        vec![
            json!({
                "FailureResource": PUBLISHED_TEMPLATES[1],
                "ExceptionType": "InvalidParameterException",
                "FailureCode": "TEMPLATE_NAME",
                "FailureDescription": "Invalid ARN.",
            }),
            json!({
                "FailureResource": PUBLISHED_TEMPLATES[1],
                "ExceptionType": "InternalFailure",
                "FailureCode": "TEMPLATE_NAME",
                "FailureDescription": TEMPLATE_NOT_FOUND,
            }),
        ],
        "expected one entry per unresolvable entry, in request order, got {put}"
    );

    let reported = listed(&router, endpoint_service).await;
    let enabled: Vec<&str> = reported
        .iter()
        .filter(|description| description.pointer("/RuleState/RuleName").is_some())
        .filter_map(|description| description.get("TemplateName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        enabled,
        vec![PUBLISHED_TEMPLATES[0], PUBLISHED_TEMPLATES[3]],
        "expected exactly the two resolvable entries enabled, got {reported:?}"
    );
}

#[tokio::test]
async fn a_required_parameter_the_request_omits_is_the_missing_parameter_fault() {
    // Rows: each managed-rule operation and the required member it was not given. The class is
    // the missing-parameter fault and the sentence names the member, so a caller repairing one
    // request reads the same form as a caller repairing the other.
    let rows: [(&str, Value, &str); 2] = [
        (
            "ListManagedInsightRules",
            json!({}),
            "MISSING_RESOURCE_ARN: The ResourceARN parameter must be present.",
        ),
        (
            "PutManagedInsightRules",
            json!({}),
            "MISSING_MANAGED_RULES: The ManagedRules parameter must be present.",
        ),
    ];

    for (operation, body, message) in &rows {
        let router = build_router();
        let (status, headers, answered) = call_in_query_mode(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "MissingRequiredParameterException"),
            "case {operation:?}: expected HTTP 400 MissingRequiredParameterException, got \
             {status} with {answered}"
        );
        assert_eq!(
            error_message(&answered),
            *message,
            "case {operation:?}: expected the sentence naming the member, got {answered}"
        );
        // The legacy query code a query-compatible client branches on, for the class and the
        // side of the fault.
        assert_eq!(
            headers
                .get("x-amzn-query-error")
                .and_then(|value| value.to_str().ok()),
            Some("MissingParameter;Sender"),
            "case {operation:?}: expected the query-error report of the class, got {headers:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused_with_its_own_sentence() {
    // Rows differ along how the token is unreadable: not the encoding this service mints, an
    // odd number of digits, and a token that decodes to bytes that are not text.
    let resource_arn = ENDPOINT_SERVICES[0].1;
    let router = build_router();
    for token in ["not-a-token", "abc", "ff"] {
        let (status, answered) = call(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource_arn, "NextToken": token}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidNextToken"),
            "case {token:?}: expected HTTP 400 InvalidNextToken, got {status} with {answered}"
        );
        assert_eq!(
            error_message(&answered),
            "INVALID_TOKEN: Unsupported token version.",
            "case {token:?}: expected the service's sentence for an unusable token, got \
             {answered}"
        );
    }
}

#[tokio::test]
async fn the_pages_of_a_listing_report_the_whole_catalog_exactly_once() {
    // Rows differ along how many descriptions one page may hold: `MaxResults`'s published
    // minimum (range 1..500), a page smaller than the catalog, one exactly as large as it, and
    // one larger — plus the default page, which holds all of it.
    let resource_arn = ENDPOINT_SERVICES[0].1;
    let router = build_router();
    ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": [managed_rule(PUBLISHED_TEMPLATES[2], resource_arn)]}),
    )
    .await;
    let whole = template_names(&listed(&router, resource_arn).await);

    for max_results in [None, Some(1), Some(3), Some(4), Some(500)] {
        let case = format!("MaxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"ResourceARN": resource_arn});
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let answered = ok(&router, "ListManagedInsightRules", &request).await;
            let page = answered
                .get("ManagedRules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(max_results) = max_results {
                assert!(
                    page.len() <= max_results,
                    "case {case:?}: a page holds at most MaxResults descriptions, got {answered}"
                );
            }
            seen.extend(template_names(&page));
            next_token = answered
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, whole,
            "case {case:?}: the pages together report the whole catalog, in its order"
        );
    }
}
