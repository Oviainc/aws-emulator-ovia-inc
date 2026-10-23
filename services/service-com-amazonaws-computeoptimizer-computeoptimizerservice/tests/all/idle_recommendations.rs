// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetIdleRecommendations`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so a response that is
//! not computed from persisted state fails here.
//!
//! An idle resource recommendation is produced by Compute Optimizer's own analysis of an
//! observed workload, and no operation of THIS API creates one, so the fixtures below
//! record recommendations into the store the way that analysis would and then read them
//! back only over the wire. What is asserted is the RULE the read follows — select by
//! resource, account and filter, sort on the requested savings dimension, page at
//! `maxResults`, and report a requested resource that has no recommendation in `errors` —
//! over a table of inputs, never one recorded call.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::{
    IdleDimension, IdleEstimatedMonthlySavings, IdleRecommendation, IdleSavingsOpportunity,
    IdleSavingsOpportunityAfterDiscounts, IdleUtilizationMetric, Tag,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, IDLE_MAX_PAGE_SIZE, IDLE_NO_RECOMMENDATION_CODE,
};

use crate::harness::{call, emulator_with_engine, error_code, opt_in, ACCOUNT};

/// The second account these fixtures use, so an `accountIds` selection has something to
/// select away.
const MEMBER_ACCOUNT: &str = "210987654321";

/// The region every ARN below names.
const REGION: &str = "us-east-1";

/// The `lastRefreshTimestamp` the recorded recommendations carry, in epoch seconds — the
/// form awsJson1_0 renders a timestamp in.
const REFRESHED_AT_SECONDS: i64 = 1_761_000_000;

/// An ARN in the grammar every AWS service shares:
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
fn arn(service: &str, account_id: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{account_id}:{resource}")
}

/// One recorded idle resource recommendation, varying along every dimension the read
/// selects, sorts or pages on.
fn recorded(
    resource_arn: &str,
    resource_id: &str,
    resource_type: &str,
    account_id: &str,
    finding: &str,
    savings: f64,
    savings_after_discounts: f64,
) -> IdleRecommendation {
    IdleRecommendation {
        resource_arn: Some(resource_arn.to_string()),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
        account_id: Some(account_id.to_string()),
        finding: Some(finding.to_string()),
        finding_description: Some(format!("The resource is {finding}.")),
        savings_opportunity: Some(IdleSavingsOpportunity {
            savings_opportunity_percentage: 25.0,
            estimated_monthly_savings: Some(IdleEstimatedMonthlySavings {
                // `Currency` models USD | CNY.
                currency: Some("USD".to_string()),
                value: savings,
            }),
        }),
        savings_opportunity_after_discounts: Some(IdleSavingsOpportunityAfterDiscounts {
            savings_opportunity_percentage: 20.0,
            estimated_monthly_savings: Some(IdleEstimatedMonthlySavings {
                currency: Some("USD".to_string()),
                value: savings_after_discounts,
            }),
        }),
        utilization_metrics: Some(vec![IdleUtilizationMetric {
            // `IdleMetricName` / `MetricStatistic` values.
            name: Some("CPU".to_string()),
            statistic: Some("Maximum".to_string()),
            value: 1.5,
            dimensions: Some(vec![IdleDimension {
                key: Some("InstanceId".to_string()),
                values: Some(vec![resource_id.to_string()]),
            }]),
        }]),
        look_back_period_in_days: 14.0,
        last_refresh_timestamp: Some(::aws_smithy_types::DateTime::from_secs(
            REFRESHED_AT_SECONDS,
        )),
        tags: Some(vec![Tag {
            key: Some("Owner".to_string()),
            value: Some("TeamA".to_string()),
        }]),
    }
}

/// The analysis output every fixture below reads: six recommendations spanning four
/// resource types, both accounts, all three `IdleFinding` values and savings figures whose
/// before- and after-discount orderings differ.
fn corpus() -> Vec<IdleRecommendation> {
    vec![
        recorded(
            &arn("ec2", ACCOUNT, "instance/i-0123456789abcdef0"),
            "i-0123456789abcdef0",
            "EC2Instance",
            ACCOUNT,
            "Idle",
            10.0,
            8.0,
        ),
        recorded(
            &arn("ec2", ACCOUNT, "volume/vol-0123456789abcdef0"),
            "vol-0123456789abcdef0",
            "EBSVolume",
            ACCOUNT,
            "Unattached",
            5.0,
            4.5,
        ),
        recorded(
            &arn("ec2", ACCOUNT, "natgateway/nat-0123456789abcdef0"),
            "nat-0123456789abcdef0",
            "NatGateway",
            ACCOUNT,
            "Unused",
            30.0,
            2.0,
        ),
        recorded(
            &arn("dynamodb", MEMBER_ACCOUNT, "table/orders"),
            "orders",
            "DynamoDBTable",
            MEMBER_ACCOUNT,
            "Idle",
            20.0,
            19.0,
        ),
        recorded(
            &arn("ecs", ACCOUNT, "service/web/checkout"),
            "checkout",
            "ECSService",
            ACCOUNT,
            "Idle",
            1.0,
            0.5,
        ),
        recorded(
            &arn("sagemaker", MEMBER_ACCOUNT, "endpoint/scoring"),
            "scoring",
            "SageMakerEndpoint",
            MEMBER_ACCOUNT,
            "Unused",
            7.5,
            7.0,
        ),
    ]
}

/// Record `recommendations` into the store `GetIdleRecommendations` reports from, the way
/// Compute Optimizer's analysis would.
fn record(engine: &Arc<engine::InMemoryEngine>, recommendations: &[IdleRecommendation]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for recommendation in recommendations {
        let resource_arn = recommendation
            .resource_arn
            .clone()
            .expect("a recorded recommendation identifies its resource");
        guard
            .idle_recommendations
            .insert(resource_arn, recommendation.clone());
    }
}

/// How many recommendations the store holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .idle_recommendations
        .len()
}

/// The `resourceArn` of every recommendation in a response, in the order reported.
fn reported_arns(body: &Value) -> Vec<String> {
    body["idleRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of idleRecommendations, got {body}"))
        .iter()
        .map(|recommendation| {
            recommendation["resourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// An emulator whose account is opted in and whose store holds [`corpus`].
async fn emulator_with_corpus() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    Vec<IdleRecommendation>,
) {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    let corpus = corpus();
    record(&engine, &corpus);
    (router, engine, corpus)
}

/// Read, asserting the call was accepted.
async fn get_idle(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, body) = call(router, "GetIdleRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// Every member the analysis recorded is reported back, unchanged, and reporting it twice
/// changes nothing a later caller sees.
#[tokio::test]
async fn a_recorded_recommendation_is_reported_member_for_member() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    let one = corpus().remove(0);
    record(&engine, std::slice::from_ref(&one));

    let body = get_idle(&router, "the recorded recommendation", json!({})).await;
    let reported = &body["idleRecommendations"][0];
    let resource_arn = one.resource_arn.clone().expect("an ARN");

    let expected = json!({
        "resourceArn": resource_arn,
        "resourceId": "i-0123456789abcdef0",
        "resourceType": "EC2Instance",
        "accountId": ACCOUNT,
        "finding": "Idle",
        "findingDescription": "The resource is Idle.",
        "savingsOpportunity": {
            "savingsOpportunityPercentage": 25.0,
            "estimatedMonthlySavings": {"currency": "USD", "value": 10.0},
        },
        "savingsOpportunityAfterDiscounts": {
            "savingsOpportunityPercentage": 20.0,
            "estimatedMonthlySavings": {"currency": "USD", "value": 8.0},
        },
        "utilizationMetrics": [{
            "name": "CPU",
            "statistic": "Maximum",
            "value": 1.5,
            "dimensions": [{"key": "InstanceId", "values": ["i-0123456789abcdef0"]}],
        }],
        "lookBackPeriodInDays": 14.0,
        "lastRefreshTimestamp": REFRESHED_AT_SECONDS,
        "tags": [{"key": "Owner", "value": "TeamA"}],
    });
    for (member, wanted) in expected.as_object().expect("an object of members") {
        assert_eq!(
            &reported[member], wanted,
            "member {member:?}: expected {wanted}, got {reported}"
        );
    }
    assert_eq!(
        body["errors"],
        json!([]),
        "no resource was named, so nothing is missing a recommendation, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "one recommendation fits on one page, got {body}"
    );

    // A read reports; it must not change what a later caller sees.
    let again = get_idle(&router, "the same read again", json!({})).await;
    assert_eq!(
        again, body,
        "a second read must report the same recommendation, got {again}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a read must not add or drop a stored recommendation"
    );
}

/// The selection rule: a request reports the stored recommendations of the resources it
/// names, of the account it names, that satisfy every filter it carries — and a member it
/// omits selects nothing away. Each row differs along one of those dimensions.
#[tokio::test]
async fn the_read_selects_by_resource_account_and_filter() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let all: Vec<String> = corpus
        .iter()
        .map(|recommendation| recommendation.resource_arn.clone().expect("an ARN"))
        .collect();
    let [instance, volume, nat_gateway, table, ecs_service, endpoint] = [0, 1, 2, 3, 4, 5]
        .map(|index| all[index].clone());

    // (case, request, the ARNs expected in the order expected)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no selection", json!({}), all.clone()),
        (
            "one resource ARN",
            json!({"resourceArns": [volume.clone()]}),
            vec![volume.clone()],
        ),
        (
            "three resource ARNs, reported in record order",
            json!({"resourceArns": [endpoint.clone(), instance.clone(), table.clone()]}),
            vec![instance.clone(), table.clone(), endpoint.clone()],
        ),
        (
            "one account",
            json!({"accountIds": [MEMBER_ACCOUNT]}),
            vec![table.clone(), endpoint.clone()],
        ),
        (
            "the other account",
            json!({"accountIds": [ACCOUNT]}),
            vec![
                instance.clone(),
                volume.clone(),
                nat_gateway.clone(),
                ecs_service.clone(),
            ],
        ),
        (
            "one finding",
            json!({"filters": [{"name": "Finding", "values": ["Unattached"]}]}),
            vec![volume.clone()],
        ),
        (
            "two findings",
            json!({"filters": [{"name": "Finding", "values": ["Unused", "Unattached"]}]}),
            vec![volume.clone(), nat_gateway.clone(), endpoint.clone()],
        ),
        (
            "one resource type",
            json!({"filters": [{"name": "ResourceType", "values": ["DynamoDBTable"]}]}),
            vec![table.clone()],
        ),
        (
            "two resource types",
            json!({"filters": [{"name": "ResourceType", "values": ["ECSService", "EBSVolume"]}]}),
            vec![volume.clone(), ecs_service.clone()],
        ),
        (
            "a finding and a resource type are ANDed",
            json!({"filters": [
                {"name": "Finding", "values": ["Idle"]},
                {"name": "ResourceType", "values": ["EC2Instance", "DynamoDBTable"]},
            ]}),
            vec![instance.clone(), table.clone()],
        ),
        (
            "a finding no stored recommendation carries",
            json!({"filters": [
                {"name": "Finding", "values": ["Unattached"]},
                {"name": "ResourceType", "values": ["EC2Instance"]},
            ]}),
            Vec::new(),
        ),
        (
            "a filter carrying no values constrains nothing",
            json!({"filters": [{"name": "Finding", "values": []}]}),
            all.clone(),
        ),
        (
            "an account and a resource ARN are ANDed",
            json!({"accountIds": [ACCOUNT], "resourceArns": [instance.clone(), table.clone()]}),
            vec![instance.clone()],
        ),
        (
            "a resource of another account",
            json!({"accountIds": [ACCOUNT], "resourceArns": [endpoint.clone()]}),
            Vec::new(),
        ),
    ];

    for (case, request, wanted) in rows {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}

/// The ordering rule: `orderBy` sorts the selection on the estimated monthly savings of the
/// dimension it names, ascending unless `Desc` asks otherwise; with no dimension to sort
/// on, the recommendations keep the order the analysis recorded them in.
///
/// The before- and after-discount figures of the corpus order differently, so a row that
/// read the wrong dimension cannot pass.
#[tokio::test]
async fn order_by_sorts_the_selection_on_the_requested_savings_dimension() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let arn_of = |index: usize| corpus[index].resource_arn.clone().expect("an ARN");

    // Savings before discounts: ecs 1.0 < volume 5.0 < endpoint 7.5 < instance 10.0 <
    // table 20.0 < nat 30.0. After discounts: nat 2.0 < ecs 0.5 … — see each row.
    let ascending_by_savings = [4, 1, 5, 0, 3, 2].map(arn_of).to_vec();
    let ascending_after_discounts = [4, 2, 1, 5, 0, 3].map(arn_of).to_vec();
    let record_order = [0, 1, 2, 3, 4, 5].map(arn_of).to_vec();

    let descending_by_savings: Vec<String> =
        ascending_by_savings.iter().rev().cloned().collect();
    let descending_after_discounts: Vec<String> =
        ascending_after_discounts.iter().rev().cloned().collect();

    // (case, orderBy, the ARNs expected in the order expected)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no orderBy", json!({}), record_order.clone()),
        (
            "SavingsValue ascending",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Asc"}}),
            ascending_by_savings.clone(),
        ),
        (
            "SavingsValue descending",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Desc"}}),
            descending_by_savings,
        ),
        (
            "SavingsValueAfterDiscount ascending",
            json!({"orderBy": {"dimension": "SavingsValueAfterDiscount", "order": "Asc"}}),
            ascending_after_discounts.clone(),
        ),
        (
            "SavingsValueAfterDiscount descending",
            json!({"orderBy": {"dimension": "SavingsValueAfterDiscount", "order": "Desc"}}),
            descending_after_discounts,
        ),
        (
            "a dimension with no order sorts ascending",
            json!({"orderBy": {"dimension": "SavingsValue"}}),
            ascending_by_savings.clone(),
        ),
        (
            "an order with no dimension keeps the record order",
            json!({"orderBy": {"order": "Desc"}}),
            record_order,
        ),
        (
            "ordering applies to the selection, not to the store",
            json!({
                "accountIds": [ACCOUNT],
                "orderBy": {"dimension": "SavingsValue", "order": "Desc"},
            }),
            [2, 0, 1, 4].map(arn_of).to_vec(),
        ),
    ];

    for (case, request, wanted) in rows {
        let body = get_idle(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}

/// The paging rule: a page carries at most `maxResults` recommendations and the `nextToken`
/// resumes exactly where it stopped, so walking the pages reports the selection once, in
/// order, whatever page size the walk uses — and no `maxResults` can exceed the ceiling the
/// model puts on it.
#[tokio::test]
async fn paging_walks_the_selection_exactly_once() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let unpaged = get_idle(&router, "unpaged", json!({})).await;
    let every_arn = reported_arns(&unpaged);
    assert_eq!(
        every_arn.len(),
        corpus.len(),
        "the whole corpus fits in one default page, got {unpaged}"
    );
    assert!(
        unpaged["nextToken"].is_null(),
        "a complete page carries no nextToken, got {unpaged}"
    );

    // `maxResults` is modelled 0..100: both published bounds, every size that splits the
    // corpus, and one size larger than it.
    for page_size in [0_u64, 1, 2, 3, 5, 6, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = get_idle(&router, &case, request).await;
            let page = reported_arns(&body);
            // `0` is the published lower bound and the service documents no meaning for
            // it, so it is read as "no size asked for" — one page of the model's ceiling.
            let cap = usize::try_from(page_size)
                .expect("a page size fits a usize")
                .clamp(1, IDLE_MAX_PAGE_SIZE);
            let cap = if page_size == 0 { IDLE_MAX_PAGE_SIZE } else { cap };
            assert!(
                page.len() <= cap,
                "case {case:?}: a page must carry at most {cap} entries, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= corpus.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, every_arn,
            "case {case:?}: the walk must report the selection exactly once, in order"
        );
    }

    // One past the published top bound is refused by the modelled range, before the
    // handler runs.
    let (status, body) = call(
        &router,
        "GetIdleRecommendations",
        json!({"maxResults": IDLE_MAX_PAGE_SIZE + 1}),
    )
    .await;
    assert_eq!(
        status, 400,
        "maxResults={}: outside the modelled range 0..{IDLE_MAX_PAGE_SIZE}, got {status}: {body}",
        IDLE_MAX_PAGE_SIZE + 1
    );
}

/// `errors` is "a list of resources that doesn't have idle recommendations": every resource
/// the request named that the store holds no recommendation for, identified by its ARN and
/// carrying the `IdleRecommendationResourceType` the ARN names.
///
/// The rows span the ARN forms of the model's own `IdleRecommendationResourceType` value
/// set, including resource types no stored recommendation covers, plus an ARN whose
/// namespace names no idle-recommendable resource — whose type is omitted rather than
/// invented.
#[tokio::test]
async fn a_requested_resource_with_no_recommendation_is_reported_in_errors() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let stored = corpus[0].resource_arn.clone().expect("an ARN");
    // The wire value of the code every entry carries, pinned here as the literal a client
    // matches on.
    assert_eq!(
        IDLE_NO_RECOMMENDATION_CODE, "ResourceNotFound",
        "the no-recommendation code is part of the wire contract"
    );

    // (case, the requested ARN, the resourceType expected on the error entry)
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "an Auto Scaling group",
            arn(
                "autoscaling",
                ACCOUNT,
                "autoScalingGroup:12345678-1234-1234-1234-123456789012:\
                 autoScalingGroupName/web-tier",
            ),
            json!("AutoScalingGroup"),
        ),
        (
            "an RDS DB instance",
            arn("rds", ACCOUNT, "db:orders"),
            json!("RDSDBInstance"),
        ),
        (
            "a DocumentDB cluster, which carries the rds namespace",
            arn("rds", ACCOUNT, "cluster:docdb-analytics"),
            json!("DocumentDBCluster"),
        ),
        (
            "an ElastiCache cluster",
            arn("elasticache", ACCOUNT, "cluster:sessions"),
            json!("ElastiCacheCluster"),
        ),
        (
            "a MemoryDB cluster",
            arn("memorydb", ACCOUNT, "cluster/leaderboard"),
            json!("MemoryDBCluster"),
        ),
        (
            "a WorkSpace",
            arn("workspaces", ACCOUNT, "workspace/ws-0123456789"),
            json!("WorkSpaces"),
        ),
        (
            "an EC2 instance the analysis never looked at",
            arn("ec2", ACCOUNT, "instance/i-00000000000000000"),
            json!("EC2Instance"),
        ),
        (
            "an ARN whose namespace names no idle-recommendable resource",
            "arn:aws:s3:::reports-bucket".to_string(),
            Value::Null,
        ),
    ];

    for (case, requested, wanted_type) in rows {
        let body = get_idle(
            &router,
            case,
            json!({"resourceArns": [requested.clone(), stored.clone()]}),
        )
        .await;
        // The stored resource is reported as a recommendation, never as an error.
        assert_eq!(
            reported_arns(&body),
            vec![stored.clone()],
            "case {case:?}: expected the stored recommendation, got {body}"
        );
        let errors = body["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list of errors, got {body}"));
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: exactly the resource with no recommendation, got {body}"
        );
        let reported = &errors[0];
        assert_eq!(
            reported["identifier"],
            json!(requested),
            "case {case:?}: the entry must identify the requested resource, got {reported}"
        );
        assert_eq!(
            reported["code"],
            json!(IDLE_NO_RECOMMENDATION_CODE),
            "case {case:?}: expected the no-recommendation code, got {reported}"
        );
        assert!(
            reported["message"].is_string(),
            "case {case:?}: the entry must say why, got {reported}"
        );
        assert_eq!(
            reported["resourceType"], wanted_type,
            "case {case:?}: expected resourceType {wanted_type}, got {reported}"
        );
    }

    // A resource the store holds puts nothing in `errors`, whatever else the request names.
    let body = get_idle(&router, "only stored resources", json!({"resourceArns": [stored]})).await;
    assert_eq!(
        body["errors"],
        json!([]),
        "a request naming only stored resources reports no errors, got {body}"
    );
}

/// A request outside the service's documented input rules is refused with HTTP 400 and
/// nothing is reported — one row per rule, each on a value the rule rejects.
#[tokio::test]
async fn an_invalid_request_is_refused() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let stored = corpus[0].resource_arn.clone().expect("an ARN");

    // A token this service really issued, taken over a different selection: resuming it
    // for another selection would report a page of a different result set.
    let other_selection = get_idle(
        &router,
        "a token over another selection",
        json!({"accountIds": [ACCOUNT], "maxResults": 1}),
    )
    .await;
    let foreign_token = other_selection["nextToken"]
        .as_str()
        .expect("a bounded page over a larger selection carries a token")
        .to_string();

    // (case, request, the wire error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            // "You can only specify one account ID per request."
            "two account ids",
            json!({"accountIds": [ACCOUNT, MEMBER_ACCOUNT]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"accountIds": ["12345"]}),
            "InvalidParameterValueException",
        ),
        (
            "a resource id that is not an ARN",
            json!({"resourceArns": ["i-0123456789abcdef0"]}),
            "InvalidParameterValueException",
        ),
        (
            "an ARN missing its resource field",
            json!({"resourceArns": [format!("arn:aws:ec2:{REGION}:{ACCOUNT}:")]}),
            "InvalidParameterValueException",
        ),
        (
            "a finding value outside the IdleFinding value set",
            json!({"filters": [{"name": "Finding", "values": ["Oversized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a resource type outside the IdleRecommendationResourceType value set",
            json!({"filters": [{"name": "ResourceType", "values": ["LambdaFunction"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({"filters": [{"values": ["Idle"]}]}),
            "InvalidParameterValueException",
        ),
        (
            // The service's prose also describes `tag:key` / `tag-key` filter names, but
            // the model's `IdleRecommendationFilterName` declares only Finding and
            // ResourceType, so the request boundary refuses anything else — the model is
            // what this emulator follows.
            "a filter name the model does not declare",
            json!({"filters": [{"name": "tag:Owner", "values": ["TeamA"]}]}),
            "ValidationException",
        ),
        (
            // Likewise `OrderBy$dimension` and `$order` are modelled enums, so a value
            // outside them never reaches the sort.
            "a sort dimension the model does not declare",
            json!({"orderBy": {"dimension": "SavingsPercentage", "order": "Asc"}}),
            "ValidationException",
        ),
        (
            "a sort order the model does not declare",
            json!({"orderBy": {"dimension": "SavingsValue", "order": "Descending"}}),
            "ValidationException",
        ),
        (
            "a forged nextToken",
            json!({"nextToken": "not-a-token"}),
            "InvalidParameterValueException",
        ),
        (
            "a nextToken issued for another selection",
            json!({"nextToken": foreign_token, "resourceArns": [stored]}),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted_code) in rows {
        let (status, body) = call(&router, "GetIdleRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains(wanted_code),
            "case {case:?}: expected {wanted_code}, got {body}"
        );
        assert!(
            body["idleRecommendations"].is_null(),
            "case {case:?}: a refused request reports nothing, got {body}"
        );
    }
}

/// The enrollment gate: "The account is not opted in to Compute Optimizer" is what a read
/// gets while the account's enrollment is not `Active`, whatever the store holds — and the
/// same read succeeds once it is.
#[tokio::test]
async fn a_read_is_refused_while_the_account_is_not_opted_in() {
    let (router, engine) = emulator_with_engine();
    record(&engine, &corpus());

    // (case, the enrollment to put the account in first, whether the read is accepted)
    let rows: Vec<(&str, Option<&str>, bool)> = vec![
        ("never enrolled", None, false),
        ("opted in", Some("Active"), true),
        ("opted back out", Some("Inactive"), false),
        ("opted in again", Some("Active"), true),
    ];

    for (case, status_to_set, accepted) in rows {
        if let Some(wanted) = status_to_set {
            let (code, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": wanted, "includeMemberAccounts": false}),
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected the enrollment update to be accepted, got {code}: {body}"
            );
        }

        let (status, body) = call(&router, "GetIdleRecommendations", json!({})).await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
            assert_eq!(
                reported_arns(&body).len(),
                corpus().len(),
                "case {case:?}: expected every recorded recommendation, got {body}"
            );
        } else {
            assert_eq!(
                status, 403,
                "case {case:?}: expected the opt-in refusal, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("OptInRequiredException"),
                "case {case:?}: expected OptInRequiredException, got {body}"
            );
        }
    }

    assert_eq!(
        stored_count(&engine),
        corpus().len(),
        "neither a refused nor an accepted read may change the store"
    );
}
