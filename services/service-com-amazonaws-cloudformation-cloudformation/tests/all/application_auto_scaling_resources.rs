// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApplicationAutoScaling::ScalableTarget` and `AWS::ApplicationAutoScaling::ScalingPolicy`
//! end to end: what a stack declares is really registered in Application Auto Scaling, and a
//! declaration that service REFUSES fails the stack.
//!
//! RULE: a declared resource of a type this emulator provisions is created by the call of the
//! service that OWNS it, so afterwards that service reports it — here
//! `DescribeScalableTargets` reports the target with the window the template declared and
//! `DescribeScalingPolicies` reports the policy attached to it — and when that service
//! refuses the call the resource fails, which leaves the stack `CREATE_FAILED` with the
//! service's own refusal as the reason rather than `CREATE_COMPLETE`.
//!
//! The rows differ along what the rule generalises over: which scalable dimension of the
//! resource is scaled, how the policy's declaration NAMES its target (the composite id a
//! `Ref` of the target resource resolves to, or the three identity members written out), which
//! kind of scaling policy is declared, and whether the target's declaration carries a
//! scheduled action. The refusal rows differ along which part of the declaration the owning
//! service refuses.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire and the Application Auto Scaling calls as a
//! client's SDK makes them — so what is asserted is what a client that deploys autoscaling by
//! template and then reads it observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{dynamodb, failure_reasons};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, REGION,
};

/// The awsJson version Application Auto Scaling's model declares (`awsJson1_1`).
const APPLICATION_AUTO_SCALING_JSON: &str = "application/x-amz-json-1.1";

/// The namespace every row scales in, and the table the stack's targets are registered on.
const NAMESPACE: &str = "dynamodb";

/// Call one Application Auto Scaling operation through the same router, signed the way a
/// client's SDK signs it: the status and the parsed answer document.
async fn application_auto_scaling(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://application-autoscaling.{REGION}.amazonaws.com/"
        ))
        .header("content-type", APPLICATION_AUTO_SCALING_JSON)
        .header(
            "x-amz-target",
            format!("AnyScaleFrontendService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                 application-autoscaling/aws4_request, SignedHeaders=host;x-amz-target, \
                 Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed Application Auto Scaling request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, document)
}

/// Create the provisioned DynamoDB table a row's targets scale, through DynamoDB's own wire:
/// a scalable target is a capacity dimension OF A RESOURCE, so the resource has to exist
/// before a target can be registered on it.
async fn given_table(router: &MultiServiceRouter, name: &str) {
    let (status, body) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateTable to answer 200, got {status} {body}"
    );
}

/// The scalable targets Application Auto Scaling reports for one resource id.
async fn scalable_targets(router: &MultiServiceRouter, resource_id: &str) -> Vec<Value> {
    let (status, document) = application_auto_scaling(
        router,
        "DescribeScalableTargets",
        &json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [resource_id]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_id:?}: expected DescribeScalableTargets to answer 200, got {status} \
         {document}"
    );
    document["ScalableTargets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The scaling policies Application Auto Scaling reports on one target.
async fn scaling_policies(
    router: &MultiServiceRouter,
    resource_id: &str,
    dimension: &str,
) -> Vec<Value> {
    let (status, document) = application_auto_scaling(
        router,
        "DescribeScalingPolicies",
        &json!({
            "ServiceNamespace": NAMESPACE,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_id:?}: expected DescribeScalingPolicies to answer 200, got {status} \
         {document}"
    );
    document["ScalingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The scheduled actions Application Auto Scaling reports on one target.
async fn scheduled_actions(
    router: &MultiServiceRouter,
    resource_id: &str,
    dimension: &str,
) -> Vec<Value> {
    let (status, document) = application_auto_scaling(
        router,
        "DescribeScheduledActions",
        &json!({
            "ServiceNamespace": NAMESPACE,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_id:?}: expected DescribeScheduledActions to answer 200, got {status} \
         {document}"
    );
    document["ScheduledActions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row: a stack declaring a scalable target (and, where the row has one, a policy on it),
/// and what the owning service must report afterwards.
struct Row {
    case: &'static str,
    stack: &'static str,
    table: &'static str,
    /// The `ScalableDimension` the row scales, a value of the model's own enum.
    dimension: &'static str,
    min: i64,
    max: i64,
    /// The policy the row declares, as `(PolicyName, the properties beside its target)`.
    policy: Option<(&'static str, Value)>,
    /// Whether the policy's declaration names its target by `Ref` of the target resource —
    /// the composite id — rather than by the three identity members.
    policy_by_reference: bool,
    /// The scheduled action the target's declaration carries, if any.
    scheduled_action: Option<Value>,
    /// Whether the declaration writes its numbers as TEXT, the way a template that quotes
    /// them — or feeds them through a `Ref` to a `String` parameter — states them.
    declared_as_text: bool,
}

/// The rows, which differ along the dimension scaled, how the policy names its target, the
/// kind of policy declared and whether a scheduled action is declared with the target.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "read capacity, with a target-tracking policy naming its target by Ref",
            stack: "aas-read-tracking",
            table: "aas-orders",
            dimension: "dynamodb:table:ReadCapacityUnits",
            min: 1,
            max: 10,
            policy: Some((
                "track-reads",
                json!({
                    "PolicyType": "TargetTrackingScaling",
                    "TargetTrackingScalingPolicyConfiguration": {
                        "TargetValue": 70.0,
                        "PredefinedMetricSpecification": {
                            "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
                        },
                    },
                }),
            )),
            policy_by_reference: true,
            scheduled_action: None,
            declared_as_text: false,
        },
        Row {
            case: "write capacity, with a step-scaling policy naming its target in full",
            stack: "aas-write-step",
            table: "aas-events",
            dimension: "dynamodb:table:WriteCapacityUnits",
            min: 2,
            max: 20,
            policy: Some((
                "step-writes",
                json!({
                    "PolicyType": "StepScaling",
                    "StepScalingPolicyConfiguration": {
                        "AdjustmentType": "ChangeInCapacity",
                        "Cooldown": 60,
                        "MetricAggregationType": "Average",
                        "StepAdjustments": [
                            {"MetricIntervalLowerBound": 0, "ScalingAdjustment": 1}
                        ],
                    },
                }),
            )),
            policy_by_reference: false,
            scheduled_action: None,
            declared_as_text: false,
        },
        Row {
            case: "a target carrying a scheduled action and no policy",
            stack: "aas-scheduled",
            table: "aas-reports",
            dimension: "dynamodb:table:ReadCapacityUnits",
            min: 3,
            max: 30,
            policy: None,
            policy_by_reference: false,
            scheduled_action: Some(json!({
                "ScheduledActionName": "nightly",
                "Schedule": "cron(0 1 * * ? *)",
                "ScalableTargetAction": {"MinCapacity": 5, "MaxCapacity": 25},
            })),
            declared_as_text: false,
        },
        Row {
            case: "a declaration whose numbers are written as text",
            stack: "aas-text-numbers",
            table: "aas-quoted",
            dimension: "dynamodb:table:ReadCapacityUnits",
            min: 4,
            max: 40,
            policy: Some((
                "track-quoted",
                json!({
                    "PolicyType": "TargetTrackingScaling",
                    "TargetTrackingScalingPolicyConfiguration": {
                        "TargetValue": "65.5",
                        "ScaleInCooldown": "120",
                        "DisableScaleIn": "true",
                        "PredefinedMetricSpecification": {
                            "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
                        },
                    },
                }),
            )),
            policy_by_reference: true,
            scheduled_action: None,
            declared_as_text: true,
        },
    ]
}

/// The `ResourceId` a row's table is scaled under.
fn resource_id(row: &Row) -> String {
    format!("table/{}", row.table)
}

/// The template one row deploys: the scalable target, the policy where the row declares one,
/// and the outputs a client reads their identifiers through.
fn template(row: &Row) -> String {
    // A number the declaration writes as TEXT is what a template that quotes it states, and
    // what a `Ref` to a `String` parameter resolves to.
    let capacity = |value: i64| {
        if row.declared_as_text {
            json!(value.to_string())
        } else {
            json!(value)
        }
    };
    let mut target = json!({
        "MinCapacity": capacity(row.min),
        "MaxCapacity": capacity(row.max),
        "ResourceId": resource_id(row),
        "ScalableDimension": row.dimension,
        "ServiceNamespace": NAMESPACE,
    });
    if let Some(action) = &row.scheduled_action {
        target["ScheduledActions"] = json!([action.clone()]);
    }
    let mut resources = json!({
        "Target": {"Type": "AWS::ApplicationAutoScaling::ScalableTarget", "Properties": target},
    });
    let mut outputs = json!({"TargetId": {"Value": {"Ref": "Target"}}});
    if let Some((policy_name, properties)) = &row.policy {
        let mut properties = properties.clone();
        properties["PolicyName"] = json!(policy_name);
        if row.policy_by_reference {
            properties["ScalingTargetId"] = json!({"Ref": "Target"});
        } else {
            properties["ResourceId"] = json!(resource_id(row));
            properties["ScalableDimension"] = json!(row.dimension);
            properties["ServiceNamespace"] = json!(NAMESPACE);
        }
        resources["Policy"] = json!({
            "Type": "AWS::ApplicationAutoScaling::ScalingPolicy",
            // A policy can only be put on a target that is already registered, so a
            // declaration that does NOT name its target by `Ref` — which is itself a
            // dependency — states the dependency itself, exactly as such a template must.
            "DependsOn": "Target",
            "Properties": properties,
        });
        outputs["PolicyArn"] = json!({"Value": {"Fn::GetAtt": ["Policy", "Arn"]}});
    }
    json!({"Resources": resources, "Outputs": outputs}).to_string()
}

/// What a stack declaring these types leaves behind: the target is registered with the window
/// the template declared, the policy is attached to it, and the composite id the stack
/// publishes for the target is the one that service identifies it by.
#[tokio::test]
async fn a_declared_scalable_target_and_policy_are_registered_in_application_auto_scaling() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let resource_id = resource_id(&row);
        given_table(&router, row.table).await;

        let (status, body) = create_stack(&router, row.stack, &template(&row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (status, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} {described}"
        );
        let stack_status = xml_text(&described, "StackStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {stack_status:?} because {:?}",
            failure_reasons(&router, row.stack).await
        );

        // The target really is registered, with the window the declaration asked for.
        let targets = scalable_targets(&router, &resource_id).await;
        let registered: Vec<&Value> = targets
            .iter()
            .filter(|target| target["ScalableDimension"] == json!(row.dimension))
            .collect();
        assert_eq!(
            registered.len(),
            1,
            "case {case:?}: expected exactly one registered target on {resource_id:?}, got \
             {targets:?}"
        );
        let target = registered[0];
        assert_eq!(
            (target["MinCapacity"].clone(), target["MaxCapacity"].clone()),
            (json!(row.min), json!(row.max)),
            "case {case:?}: expected the declared capacity window, got {target:?}"
        );

        // The id the stack publishes for the target is the composite that service identifies
        // it by, which is also what a policy's `ScalingTargetId` takes.
        let published = xml_text(&described, "OutputValue");
        assert!(
            published.is_some(),
            "case {case:?}: expected the stack to publish the target's id, got none"
        );
        let expected_id = format!("{resource_id}|{}|{NAMESPACE}", row.dimension);
        assert!(
            described.contains(&expected_id),
            "case {case:?}: expected the stack to publish the composite id {expected_id:?}, \
             got {described}"
        );

        // The policy the row declares is attached to that target, under the name it declared.
        let policies = scaling_policies(&router, &resource_id, row.dimension).await;
        match &row.policy {
            Some((policy_name, properties)) => {
                let names: Vec<&str> = policies
                    .iter()
                    .filter_map(|policy| policy["PolicyName"].as_str())
                    .collect();
                assert_eq!(
                    names,
                    vec![*policy_name],
                    "case {case:?}: expected the declared policy on the target, got {policies:?}"
                );
                assert_eq!(
                    policies[0]["PolicyType"], properties["PolicyType"],
                    "case {case:?}: expected the declared PolicyType, got {:?}",
                    policies[0]
                );
            }
            None => assert!(
                policies.is_empty(),
                "case {case:?}: expected no scaling policy on the target, got {policies:?}"
            ),
        }

        // A scheduled action declared with the target is stored on it too.
        let actions = scheduled_actions(&router, &resource_id, row.dimension).await;
        match &row.scheduled_action {
            Some(declared) => {
                let names: Vec<&str> = actions
                    .iter()
                    .filter_map(|action| action["ScheduledActionName"].as_str())
                    .collect();
                assert_eq!(
                    names,
                    vec![declared["ScheduledActionName"].as_str().unwrap_or_default()],
                    "case {case:?}: expected the declared scheduled action, got {actions:?}"
                );
            }
            None => assert!(
                actions.is_empty(),
                "case {case:?}: expected no scheduled action on the target, got {actions:?}"
            ),
        }

        // Deleting the stack takes the target away in the service that holds it.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (_, _) = describe_stack(&router, row.stack).await;
        let left = scalable_targets(&router, &resource_id).await;
        assert!(
            left.iter()
                .all(|target| target["ScalableDimension"] != json!(row.dimension)),
            "case {case:?}: expected the stack's deletion to deregister the target, got {left:?}"
        );
    }
}

/// One refusal row: a declaration the owning service will not accept, and the sentence its
/// refusal carries.
struct RefusedRow {
    case: &'static str,
    stack: &'static str,
    table: Option<&'static str>,
    /// The `AWS::ApplicationAutoScaling::ScalableTarget` properties the template declares.
    properties: Value,
    /// Text the stack's failure reason must carry: the owning service's own refusal.
    reason: &'static str,
}

/// A declaration Application Auto Scaling refuses fails the resource, and with it the stack:
/// nothing is registered and the stack ends `CREATE_FAILED` carrying that service's refusal.
///
/// The rows differ along which part of the declaration is refused: an inverted capacity
/// window, and a resource that does not exist in the service that owns it.
#[tokio::test]
async fn a_declaration_the_owning_service_refuses_fails_the_stack() {
    let rows: &[RefusedRow] = &[
        RefusedRow {
            case: "MinCapacity above MaxCapacity",
            stack: "aas-inverted-window",
            table: Some("aas-inverted"),
            properties: json!({
                "MinCapacity": 9,
                "MaxCapacity": 8,
                "ResourceId": "table/aas-inverted",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "ServiceNamespace": NAMESPACE,
            }),
            reason: "minimum capacity",
        },
        RefusedRow {
            case: "a table that does not exist",
            stack: "aas-missing-table",
            table: None,
            properties: json!({
                "MinCapacity": 1,
                "MaxCapacity": 5,
                "ResourceId": "table/aas-never-created",
                "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
                "ServiceNamespace": NAMESPACE,
            }),
            reason: "DynamoDB table does not exist",
        },
    ];

    for row in rows {
        let router = router();
        let case = row.case;
        if let Some(table) = row.table {
            given_table(&router, table).await;
        }
        let template = json!({
            "Resources": {
                "Target": {
                    "Type": "AWS::ApplicationAutoScaling::ScalableTarget",
                    "Properties": row.properties.clone(),
                },
            },
        })
        .to_string();
        let (status, body) = create_stack(
            &router,
            row.stack,
            &template,
            // The failed state is KEPT rather than rolled back, so what the stack reports is
            // the failure itself: "DO_NOTHING — if the stack creation fails, do nothing".
            &[member("OnFailure", "DO_NOTHING")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        let stack_status = xml_text(&described, "StackStatus");
        assert_eq!(
            stack_status.as_deref(),
            Some("CREATE_FAILED"),
            "case {case:?}: expected the refused declaration to fail the stack, got \
             {stack_status:?}"
        );
        let reasons = failure_reasons(&router, row.stack).await;
        assert!(
            reasons.iter().any(|reason| reason.contains(row.reason)),
            "case {case:?}: expected a failure reason carrying {:?}, got {reasons:?}",
            row.reason
        );

        // Nothing was registered: a refused create leaves no target behind.
        let resource_id = row.properties["ResourceId"].as_str().unwrap_or_default();
        let left = scalable_targets(&router, resource_id).await;
        assert!(
            left.is_empty(),
            "case {case:?}: expected a refused declaration to register nothing, got {left:?}"
        );
    }
}

/// A stack whose target declaration is refused still reports the resources it DID create: the
/// table the same template declares — and that the target's own declaration references — is
/// created before the target is attempted, so the stack reports it `CREATE_COMPLETE` beside
/// the `CREATE_FAILED` target.
#[tokio::test]
async fn a_failed_target_leaves_the_resources_the_stack_did_create_reported() {
    let router = router();
    let stack = "aas-partial-stack";
    let template = json!({
        "Resources": {
            "Table": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": "aas-partial",
                    "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                    "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
                },
            },
            "Target": {
                "Type": "AWS::ApplicationAutoScaling::ScalableTarget",
                "Properties": {
                    // Refused by the owning service: a window whose minimum is above its
                    // maximum is no window at all.
                    "MinCapacity": 9,
                    "MaxCapacity": 8,
                    // The table the same stack declares, which is what makes the target
                    // depend on it.
                    "ResourceId": {"Fn::Sub": ["table/${Name}", {"Name": {"Ref": "Table"}}]},
                    "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                    "ServiceNamespace": NAMESPACE,
                },
            },
        },
    })
    .to_string();
    let (status, body) = create_stack(
        &router,
        stack,
        &template,
        &[member("OnFailure", "DO_NOTHING")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a stack whose target is refused\": expected HTTP 200 from CreateStack, got \
         {status} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, stack).await;
    let stack_status = xml_text(&described, "StackStatus");
    assert_eq!(
        stack_status.as_deref(),
        Some("CREATE_FAILED"),
        "case \"a stack whose target is refused\": expected the stack to fail, got \
         {stack_status:?}"
    );

    let (_, resources) = call(
        &router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    let reported: Vec<(String, String)> = xml_blocks(&resources, "member")
        .into_iter()
        .filter_map(|resource| {
            Some((
                xml_text(&resource, "LogicalResourceId")?,
                xml_text(&resource, "ResourceStatus")?,
            ))
        })
        .collect();
    for (logical_id, expected) in [("Table", "CREATE_COMPLETE"), ("Target", "CREATE_FAILED")] {
        let status = reported
            .iter()
            .find(|(reported_id, _)| reported_id == logical_id)
            .map(|(_, status)| status.as_str());
        assert_eq!(
            status,
            Some(expected),
            "case {logical_id:?}: expected the stack to report it {expected}, got {reported:?}"
        );
    }
}
