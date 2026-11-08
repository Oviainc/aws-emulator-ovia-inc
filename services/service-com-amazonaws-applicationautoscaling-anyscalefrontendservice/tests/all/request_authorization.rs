// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven over the real wire path (an awsJson1_1
//! request through the multi-service router into the mounted service), so what is asserted is
//! what a client sees.
//!
//! RULE: every operation of this service evaluates the identity-based IAM policies in force
//! on the principal the request's credentials name — against
//! `application-autoscaling:<OperationName>`, the resource the request addresses and the
//! condition keys that request carries — BEFORE it reads or writes anything. A request no
//! statement allows, one an explicit `Deny` covers, one whose grant a condition does not
//! hold for, and one whose permissions boundary withholds the grant are each refused with
//! `AccessDeniedException` and perform NOTHING; a request a matching `Allow` covers, and one
//! presenting the account's own published credential, are served. A registration that carries
//! `Tags` needs the tagging permission as well.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal is the one `sts:GetCallerIdentity`
//! answers for the key the request signed with.
//!
//! The tables span what the rule generalises over: the operation (a read, a create, a policy
//! write and a delete), the document in force (none at all, another service's actions, an
//! explicit deny beside an allow of everything, an allow of exactly the action, an allow of
//! the whole service, a grant conditioned on the scalable dimension that does and does not
//! hold, a grant conditioned on the addressed target's tags that does and does not hold), the
//! permissions boundary (none, one that allows the action, one that allows another service
//! only), and the credential (the session, and the account's own).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{error_code, error_message, ACCOUNT, REGION};

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEMULATOR";

/// The access key id of the assumed-role session the rows sign with.
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION01";

/// The role that session is assumed from, the session's own name, and the name of the inline
/// policy the role carries.
const ROLE: &str = "aas-operator";
const SESSION: &str = "aas-session";
const POLICY_NAME: &str = "inline";

/// How long the seeded session lasts: an hour, STS's own default.
const SESSION_SECONDS: i32 = 3_600;

/// Two managed policies the IAM emulator's own catalog publishes a document for, so attaching
/// either as a permissions boundary is a ceiling this emulator CAN read: one that allows every
/// action, and one that allows another service's actions only.
const BOUNDARY_ALLOWING_EVERYTHING: &str = "arn:aws:iam::aws:policy/AdministratorAccess";
const BOUNDARY_ALLOWING_ANOTHER_SERVICE: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// The table every row's target scales, and the identity of that target.
const TABLE: &str = "orders";
const RESOURCE_ID: &str = "table/orders";
const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";
const NAMESPACE: &str = "dynamodb";

/// A router serving this service beside DynamoDB (where the scaled table lives), CloudWatch
/// (where a target-tracking policy's alarms live) and the IAM and STS services, over ONE
/// shared engine — so the policies and the session the cases seed are the ones those services
/// hold.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// Dispatch one awsJson request signed with `access_key_id` for `signing_name`.
async fn dispatch_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    (status, document)
}

/// Call one Application Auto Scaling operation as the principal `access_key_id` names.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    dispatch_as(
        router,
        access_key_id,
        "application-autoscaling",
        "application/x-amz-json-1.1",
        &format!("AnyScaleFrontendService.{operation}"),
        body,
    )
    .await
}

/// Create the provisioned DynamoDB table the rows' target scales, as the account.
async fn given_table(router: &MultiServiceRouter) {
    let (status, body) = dispatch_as(
        router,
        ACCOUNT_KEY_ID,
        "dynamodb",
        "application/x-amz-json-1.0",
        "DynamoDB_20120810.CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the table the rows scale\": expected CreateTable to answer 200, got {status} \
         {body:?}"
    );
}

/// Register the target the rows act on, as the account, with the tags a resource-tag
/// condition is written against; answer the ARN this service minted for it.
async fn given_target(router: &MultiServiceRouter) -> String {
    let (status, body) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "RegisterScalableTarget",
        &json!({
            "ServiceNamespace": NAMESPACE,
            "ResourceId": RESOURCE_ID,
            "ScalableDimension": DIMENSION,
            "MinCapacity": 1,
            "MaxCapacity": 10,
            "Tags": {"team": "beta"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the target the rows act on\": expected RegisterScalableTarget to answer 200, \
         got {status} {body:?}"
    );
    body["ScalableTargetARN"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ScalableTargetARN, got {body:?}"))
        .to_string()
}

/// The scalable targets this service reports for `resource_id`, read as the account.
async fn targets_of(router: &MultiServiceRouter, resource_id: &str) -> Vec<Value> {
    let (status, body) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "DescribeScalableTargets",
        &json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [resource_id]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_id:?}: expected DescribeScalableTargets to answer 200, got {status} \
         {body:?}"
    );
    body["ScalableTargets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARN the account holds the role under, which is the identity whose policies an
/// assumed-role session's request is judged on.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// The principal ARN the seeded session speaks for — what a refusal must name.
fn session_arn() -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{SESSION}")
}

/// Record the role and the assumed-role session [`SESSION_KEY_ID`] speaks for, as
/// `iam:CreateRole` and `sts:AssumeRole` record them.
fn given_session(engine: &Arc<engine::InMemoryEngine>) {
    let role_arn = role_arn();
    let iam_store = iam::service_state(&**engine).expect("the IAM store");
    iam_store.lock().roles.insert(
        iam::name_key(ROLE),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: ROLE.to_string(),
            role_id: "AROAEXAMPLEAASROLE001".to_string(),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let store = sts::service_state(&**engine).expect("the STS store");
    store
        .lock()
        .insert(vend.into_session(SESSION_KEY_ID, SystemTime::now()))
        .expect("the session is recorded");
}

/// Attach `document` to the role as its inline policy, as `iam:PutRolePolicy` does; `None`
/// leaves the role with no policy at all.
fn given_role_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(ROLE));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(ROLE),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}

/// Attach `arn` to the role as its PERMISSIONS BOUNDARY, the ceiling an identity-based grant
/// cannot exceed — what `iam:PutRolePermissionsBoundary` does.
fn given_role_boundary(engine: &Arc<engine::InMemoryEngine>, arn: Option<&str>) {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::AttachedPermissionsBoundary;
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    let role = store
        .roles
        .get_mut(&iam::name_key(ROLE))
        .unwrap_or_else(|| panic!("the IAM store holds no role {ROLE}"));
    role.permissions_boundary = arn.map(|arn| AttachedPermissionsBoundary {
        permissions_boundary_type: Some("Policy".to_string()),
        permissions_boundary_arn: Some(arn.to_string()),
    });
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}]
    })
}

/// A policy allowing every action of this service under `condition`.
fn allow_under(condition: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "application-autoscaling:*",
            "Resource": "*",
            "Condition": condition,
        }]
    })
}

/// Which ARN IAM judges a row's request against: the target's own where the request names all
/// three identity members of a target this service holds, and the Region's whole collection
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressed {
    /// Every scalable target of the Region.
    Collection,
    /// The target the rows act on.
    Target,
}

/// One row: the operation a principal calls, the document in force, the boundary that caps it
/// and whether the request is served.
struct Row {
    case: &'static str,
    /// The access key id the request is signed with.
    key_id: &'static str,
    operation: &'static str,
    /// The request body, beside the identity members the helper adds.
    body: fn() -> Value,
    addressed: Addressed,
    /// The inline policy in force, or `None` for a principal with no policy at all.
    document: Option<fn() -> Value>,
    /// The permissions boundary in force.
    boundary: Option<&'static str>,
    /// `None` where the request is served; otherwise the clause the refusal closes with.
    refused_because: Option<&'static str>,
}

/// The request body of each operation the rows span, over the ONE target they act on.
fn describe_body() -> Value {
    json!({"ServiceNamespace": NAMESPACE, "ResourceIds": [RESOURCE_ID]})
}
fn register_body() -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": RESOURCE_ID,
        "ScalableDimension": DIMENSION,
        "MinCapacity": 2,
        "MaxCapacity": 20,
    })
}
fn put_policy_body() -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": RESOURCE_ID,
        "ScalableDimension": DIMENSION,
        "PolicyName": "track",
        "PolicyType": "TargetTrackingScaling",
        "TargetTrackingScalingPolicyConfiguration": {
            "TargetValue": 70.0,
            "PredefinedMetricSpecification": {
                "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
            },
        },
    })
}
fn deregister_body() -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": RESOURCE_ID,
        "ScalableDimension": DIMENSION,
    })
}

/// The clauses an authorization refusal closes with, per IAM rule.
const NO_POLICY_ALLOWS: &str = "because no identity-based policy allows the";
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";
const NO_BOUNDARY_ALLOWS: &str = "because no permissions boundary allows the";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a read with no policy at all",
            key_id: SESSION_KEY_ID,
            operation: "DescribeScalableTargets",
            body: describe_body,
            addressed: Addressed::Collection,
            document: None,
            boundary: None,
            refused_because: Some(NO_POLICY_ALLOWS),
        },
        Row {
            case: "a read allowed by a grant of the whole service",
            key_id: SESSION_KEY_ID,
            operation: "DescribeScalableTargets",
            body: describe_body,
            addressed: Addressed::Collection,
            document: Some(|| allow(json!("application-autoscaling:*"))),
            boundary: None,
            refused_because: None,
        },
        Row {
            case: "a read allowed by a grant of exactly that action",
            key_id: SESSION_KEY_ID,
            operation: "DescribeScalableTargets",
            body: describe_body,
            addressed: Addressed::Collection,
            document: Some(|| allow(json!("application-autoscaling:DescribeScalableTargets"))),
            boundary: None,
            refused_because: None,
        },
        Row {
            case: "a read whose only grant is of another service",
            key_id: SESSION_KEY_ID,
            operation: "DescribeScalableTargets",
            body: describe_body,
            addressed: Addressed::Collection,
            document: Some(|| allow(json!("dynamodb:*"))),
            boundary: None,
            refused_because: Some(NO_POLICY_ALLOWS),
        },
        Row {
            case: "the account's own published credential, which holds no policies",
            key_id: ACCOUNT_KEY_ID,
            operation: "DescribeScalableTargets",
            body: describe_body,
            addressed: Addressed::Collection,
            document: None,
            boundary: None,
            refused_because: None,
        },
        Row {
            case: "a registration an explicit deny covers",
            key_id: SESSION_KEY_ID,
            operation: "RegisterScalableTarget",
            body: register_body,
            addressed: Addressed::Target,
            document: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {"Effect": "Allow", "Action": "*", "Resource": "*"},
                        {
                            "Effect": "Deny",
                            "Action": "application-autoscaling:RegisterScalableTarget",
                            "Resource": "*",
                        },
                    ]
                })
            }),
            boundary: None,
            refused_because: Some(EXPLICIT_DENY),
        },
        Row {
            case: "a registration allowed for the dimension it names",
            key_id: SESSION_KEY_ID,
            operation: "RegisterScalableTarget",
            body: register_body,
            addressed: Addressed::Target,
            document: Some(|| {
                allow_under(json!({
                    "StringEquals": {"application-autoscaling:scalable-dimension": DIMENSION}
                }))
            }),
            boundary: None,
            refused_because: None,
        },
        Row {
            case: "a registration allowed only for another dimension",
            key_id: SESSION_KEY_ID,
            operation: "RegisterScalableTarget",
            body: register_body,
            addressed: Addressed::Target,
            document: Some(|| {
                allow_under(json!({
                    "StringEquals": {
                        "application-autoscaling:scalable-dimension":
                            "dynamodb:table:WriteCapacityUnits"
                    }
                }))
            }),
            boundary: None,
            refused_because: Some(NO_POLICY_ALLOWS),
        },
        Row {
            case: "a registration allowed only in another namespace",
            key_id: SESSION_KEY_ID,
            operation: "RegisterScalableTarget",
            body: register_body,
            addressed: Addressed::Target,
            document: Some(|| {
                allow_under(json!({
                    "StringEquals": {"application-autoscaling:service-namespace": "ecs"}
                }))
            }),
            boundary: None,
            refused_because: Some(NO_POLICY_ALLOWS),
        },
        Row {
            case: "a policy write a grant of the whole service allows",
            key_id: SESSION_KEY_ID,
            operation: "PutScalingPolicy",
            body: put_policy_body,
            addressed: Addressed::Target,
            document: Some(|| allow(json!("application-autoscaling:*"))),
            boundary: None,
            refused_because: None,
        },
        Row {
            case: "a policy write whose grant the boundary withholds",
            key_id: SESSION_KEY_ID,
            operation: "PutScalingPolicy",
            body: put_policy_body,
            addressed: Addressed::Target,
            document: Some(|| allow(json!("application-autoscaling:*"))),
            boundary: Some(BOUNDARY_ALLOWING_ANOTHER_SERVICE),
            refused_because: Some(NO_BOUNDARY_ALLOWS),
        },
        Row {
            case: "a policy write the boundary allows too",
            key_id: SESSION_KEY_ID,
            operation: "PutScalingPolicy",
            body: put_policy_body,
            addressed: Addressed::Target,
            document: Some(|| allow(json!("application-autoscaling:*"))),
            boundary: Some(BOUNDARY_ALLOWING_EVERYTHING),
            refused_because: None,
        },
        Row {
            case: "a deletion allowed only for another value of the target's tag",
            key_id: SESSION_KEY_ID,
            operation: "DeregisterScalableTarget",
            body: deregister_body,
            addressed: Addressed::Target,
            document: Some(|| {
                allow_under(json!({"StringEquals": {"aws:ResourceTag/team": "alpha"}}))
            }),
            boundary: None,
            refused_because: Some(NO_POLICY_ALLOWS),
        },
        Row {
            case: "a deletion allowed for the tag the target carries",
            key_id: SESSION_KEY_ID,
            operation: "DeregisterScalableTarget",
            body: deregister_body,
            addressed: Addressed::Target,
            document: Some(|| {
                allow_under(json!({"StringEquals": {"aws:ResourceTag/team": "beta"}}))
            }),
            boundary: None,
            refused_because: None,
        },
    ]
}

#[tokio::test]
async fn a_request_is_served_only_where_the_principals_policies_allow_the_action() {
    for row in rows() {
        let case = row.case;
        // One emulator per row, so what a served request changed cannot reach the next row.
        let (router, engine) = router_with_identity();
        given_table(&router).await;
        let target_arn = given_target(&router).await;
        given_session(&engine);
        let document = row.document.map(|build| build());
        given_role_policy(&engine, document.as_ref());
        given_role_boundary(&engine, row.boundary);

        let (status, body) = call_as(&router, row.key_id, row.operation, &(row.body)()).await;
        let Some(reason) = row.refused_because else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the request to be served, got {status} {body:?}"
            );
            continue;
        };

        // The refusal: this service's own access-denied code and status, carrying IAM's
        // sentence about the principal the credential speaks for.
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body:?}"
        );
        let addressed = match row.addressed {
            Addressed::Collection => {
                format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/*")
            }
            Addressed::Target => target_arn.clone(),
        };
        let expected = format!(
            "User: {} is not authorized to perform: application-autoscaling:{} on resource: \
             {addressed} {reason}",
            session_arn(),
            row.operation,
        );
        let message = error_message(&body);
        let expected = if reason == EXPLICIT_DENY {
            expected
        } else {
            format!(
                "{expected} application-autoscaling:{} action",
                row.operation
            )
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The refused call performed NOTHING: the target the rows act on is still registered
        // with the window the account registered it under, and no policy was written.
        let targets = targets_of(&router, RESOURCE_ID).await;
        assert_eq!(
            targets.len(),
            1,
            "case {case:?}: expected the registered target to survive the refusal, got \
             {targets:?}"
        );
        assert_eq!(
            (
                targets[0]["MinCapacity"].clone(),
                targets[0]["MaxCapacity"].clone()
            ),
            (json!(1), json!(10)),
            "case {case:?}: expected the refused call to apply nothing, got {:?}",
            targets[0]
        );
        let (status, listed) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            "DescribeScalingPolicies",
            &json!({
                "ServiceNamespace": NAMESPACE,
                "ResourceId": RESOURCE_ID,
                "ScalableDimension": DIMENSION,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} \
             {listed:?}"
        );
        assert_eq!(
            listed["ScalingPolicies"],
            json!([]),
            "case {case:?}: expected the refused call to write no policy, got {listed:?}"
        );
    }
}

/// RULE: a registration that carries `Tags` needs the TAGGING permission as well, so a policy
/// that grants only the registration does not cover it — and the refused call registers
/// nothing.
///
/// The rows differ along which permissions the document carries, and the last one carries no
/// tags at all, so what the registration needs follows the REQUEST rather than the operation.
#[tokio::test]
async fn a_registration_carrying_tags_also_needs_the_tagging_permission() {
    // (case, the actions granted, whether the request carries tags, whether it is served)
    let rows: &[(&str, Value, bool, bool)] = &[
        (
            "tags, with the registration permission alone",
            json!("application-autoscaling:RegisterScalableTarget"),
            true,
            false,
        ),
        (
            "tags, with the tagging permission too",
            json!([
                "application-autoscaling:RegisterScalableTarget",
                "application-autoscaling:TagResource",
            ]),
            true,
            true,
        ),
        (
            "no tags, with the registration permission alone",
            json!("application-autoscaling:RegisterScalableTarget"),
            false,
            true,
        ),
    ];

    for (case, actions, carries_tags, served) in rows {
        let (router, engine) = router_with_identity();
        given_table(&router).await;
        given_session(&engine);
        given_role_policy(&engine, Some(&allow(actions.clone())));

        let mut request = register_body();
        if *carries_tags {
            request["Tags"] = json!({"team": "beta"});
        }
        let (status, body) = call_as(&router, SESSION_KEY_ID, "RegisterScalableTarget", &request)
            .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to be served, got {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("application-autoscaling:TagResource"),
            "case {case:?}: expected the refusal to name the tagging action, got {message:?}"
        );
        // Nothing was registered: a refused call performs none of what it asked for.
        let targets = targets_of(&router, RESOURCE_ID).await;
        assert!(
            targets.is_empty(),
            "case {case:?}: expected the refused registration to register nothing, got \
             {targets:?}"
        );
    }
}
