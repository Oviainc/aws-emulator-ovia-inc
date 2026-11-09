// SPDX-License-Identifier: CC0-1.0

//! Cover for the `Definition` a MANAGED Contributor Insights rule carries.
//!
//! The rule under test: **a managed rule's definition names, under `Resource`, the ARN of the
//! Amazon Web Services resource whose data the rule collects, beside the managed-rule schema
//! and the template it was enabled from** — "When you enable a managed rule, you create a
//! Contributor Insights rule that collects data from Amazon Web Services services. You cannot
//! edit these rules with PutInsightRule." (`PutManagedInsightRules`), and the rule it creates
//! is reported with "the definition of the rule, as a JSON object"
//! (`smithy_query(query="shape",shape="InsightRule")`, member `Definition`) plus "an optional
//! built-in rule that Amazon Web Services manages" (`InsightRule$ManagedRule`) and the schema
//! "`{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`). A client that reads
//! the described rules tells WHICH resource a managed rule collects from by that member, so
//! every managed rule this service enables carries it, whatever template and whatever resource
//! it was enabled for.
//!
//! Every table row is a distinct input along what the rule generalises over: which half of the
//! catalog published the template (the service's own, and this account's), the kind of resource
//! the template applies to, and the Region, account, partition and id form of its ARN.

use crate::harness::{build_router_with_engine, ok, publish_managed_rule_template};
use serde_json::{json, Value};

/// One managed-rule template the SERVICE publishes for a VPC endpoint service
/// (`crate::state::insight_rule_records`'s catalog), as `ListManagedInsightRules` reports
/// them — a template no row below publishes, so a row naming one exercises the service's own
/// half of the catalog.
const SERVICE_TEMPLATE: &str = "VpcEndpointService-BytesByEndpointId-v1";

/// What the enabled rule's `Schema` member reports for a managed rule: "For managed rules, this
/// is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`), as the compact schema
/// that value names.
const MANAGED_SCHEMA: &str = "ServiceLogRule/1";

/// One row: the template a managed rule is enabled from, the resource it collects data for, and
/// whether the template has to be published into this account's half of the catalog first (the
/// service publishes its own).
struct Row {
    /// What the row is, for the case name every assertion carries.
    case: &'static str,
    /// The template name the request carries.
    template: &'static str,
    /// The `ResourceARN` the request carries.
    resource_arn: &'static str,
    /// Whether the row publishes `template` for `resource_arn` before enabling it.
    publish: bool,
}

/// The rows: both halves of the catalog, four kinds of resource, and every ARN form AWS issues
/// one in — different Regions, accounts, partitions and id shapes, including ARNs whose Region
/// and account are empty.
const ROWS: &[Row] = &[
    Row {
        case: "a service-published template for a VPC endpoint service with a long id",
        template: SERVICE_TEMPLATE,
        resource_arn: "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0123456789abcdef0",
        publish: false,
    },
    Row {
        case: "the same template in another Region, account and partition",
        template: "VpcEndpointService-ActiveConnectionsByEndpointId-v1",
        resource_arn: "arn:aws-cn:ec2:cn-north-1:210987654321:vpc-endpoint-service/vpce-svc-0abc1234",
        publish: false,
    },
    Row {
        case: "an account-published template for a DynamoDB table",
        template: "ServiceLogRule-DynamoDB",
        resource_arn: "arn:aws:dynamodb:eu-west-3:123456789012:table/Orders",
        publish: true,
    },
    Row {
        case: "an account-published template for an S3 bucket with neither Region nor account",
        template: "S3-RequestsByRequester-v2",
        resource_arn: "arn:aws:s3:::access-logs",
        publish: true,
    },
    Row {
        case: "an account-published template whose name carries no version",
        template: "Lambda-Insights",
        resource_arn: "arn:aws:lambda:us-west-2:000000000000:function:checkout",
        publish: true,
    },
];

#[tokio::test]
async fn a_managed_rule_definition_names_the_resource_it_collects_from() {
    for row in ROWS {
        let (router, engine) = build_router_with_engine();
        if row.publish {
            publish_managed_rule_template(&engine, row.template, row.resource_arn);
        }

        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({
                "ManagedRules": [{
                    "TemplateName": row.template,
                    "ResourceARN": row.resource_arn,
                }],
            }),
        )
        .await;
        assert_eq!(
            put.get("Failures").and_then(Value::as_array),
            Some(&Vec::<Value>::new()),
            "case {:?}: enabling a published template reports no failure, got {put}",
            row.case
        );

        // The rule reads back through DescribeInsightRules, which is where a client reads a
        // managed rule's definition.
        let described = ok(&router, "DescribeInsightRules", &json!({})).await;
        let rules = described
            .get("InsightRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rules.len(),
            1,
            "case {:?}: expected the one enabled managed rule back, got {described}",
            row.case
        );
        let rule = &rules[0];
        assert_eq!(
            (
                rule.get("ManagedRule").and_then(Value::as_bool),
                rule.get("Schema").and_then(Value::as_str),
            ),
            (Some(true), Some(MANAGED_SCHEMA)),
            "case {:?}: a managed rule reports the managed schema, got {rule}",
            row.case
        );

        let text = rule
            .get("Definition")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let definition: Value = serde_json::from_str(text).unwrap_or_else(|error| {
            panic!(
                "case {:?}: a rule's definition is a JSON object, got {text:?} ({error})",
                row.case
            )
        });
        assert_eq!(
            definition.get("Resource").and_then(Value::as_str),
            Some(row.resource_arn),
            "case {:?}: the definition names the resource it collects from, got {definition}",
            row.case
        );
        assert_eq!(
            definition.get("TemplateName").and_then(Value::as_str),
            Some(row.template),
            "case {:?}: the definition names the template it was enabled from, got {definition}",
            row.case
        );
        assert_eq!(
            definition.get("Schema"),
            Some(&json!({"Name": "ServiceLogRule", "Version": 1})),
            "case {:?}: the definition declares the managed-rule schema, got {definition}",
            row.case
        );

        // A read does not rewrite it: the same definition is reported again.
        let again = ok(&router, "DescribeInsightRules", &json!({})).await;
        assert_eq!(
            again, described,
            "case {:?}: a second read reports the same rules, got {again}",
            row.case
        );
    }
}

#[tokio::test]
async fn re_enabling_a_managed_rule_leaves_its_definition_alone() {
    // "If a previously created managed rule is currently disabled, a subsequent call to this
    // API will re-enable it." (`PutManagedInsightRules`) — re-enabling names the same rule, so
    // the definition the first call wrote is the one a later read still reports.
    let resource_arn =
        "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0123456789abcdef0";
    let (router, _engine) = build_router_with_engine();
    let request = json!({
        "ManagedRules": [{"TemplateName": SERVICE_TEMPLATE, "ResourceARN": resource_arn}],
    });

    ok(&router, "PutManagedInsightRules", &request).await;
    let first = ok(&router, "DescribeInsightRules", &json!({})).await;
    let name = first
        .pointer("/InsightRules/0/Name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    ok(
        &router,
        "DisableInsightRules",
        &json!({"RuleNames": [&name]}),
    )
    .await;
    ok(&router, "PutManagedInsightRules", &request).await;

    let after = ok(&router, "DescribeInsightRules", &json!({})).await;
    assert_eq!(
        after.pointer("/InsightRules/0/Definition"),
        first.pointer("/InsightRules/0/Definition"),
        "case \"re-enabled\": the definition is left alone, got {after}"
    );
    assert_eq!(
        after.pointer("/InsightRules/0/State").and_then(Value::as_str),
        Some("ENABLED"),
        "case \"re-enabled\": the rule is enabled again, got {after}"
    );
}
