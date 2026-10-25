// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Cognito::UserPoolGroup` really creates the
//! group in Amazon Cognito, and the handle the stack hands back is the one that service
//! answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the stack
//! publishes for it is the one that type's published return values document — for a
//! user-pool group, the group NAME the owning service holds — so `Ref` resolves to a value
//! `cognito-idp GetGroup(UserPoolId, GroupName)` resolves to the group the template
//! declared.** Two crates have to agree for that to hold (CloudFormation makes the call,
//! Amazon Cognito serves it), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! group or leaves CloudFormation to generate its name, which of the three configuration
//! properties it declares (`Description`, `Precedence`, `RoleArn`) and in which spelling a
//! number-valued one arrives (a JSON number, a quoted YAML scalar, a `String` template
//! parameter), whether the pool it names is a literal property or reaches the resource
//! through a template `Parameter`, whether the template is written in JSON or YAML, and
//! which intrinsic an output reads the resource with (`Ref`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none,
//! which is CloudFormation's to compute: what is checked about it is that it is a name
//! inside the bound the owning model publishes for one and that Amazon Cognito resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The longest group name Amazon Cognito accepts: `GroupNameType`,
/// `@length(min: 1, max: 128)` in that service's own Smithy model.
const MAX_GROUP_NAME: usize = 128;

/// An IAM role ARN of the form `CreateGroup.RoleArn` admits (`ArnType`, length 20..2048).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/cognito-group-role";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::
            EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon Cognito user-pools operation over its awsJson1_1 wire, signed for
/// `region`.
async fn cognito(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", authorization("cognito-idp", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A user pool a CLIENT created, as that service minted its id — the pool a stack's group
/// is declared in, exactly as a caller would hand it to a template.
async fn client_user_pool(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, created) = cognito(
        router,
        region,
        "CreateUserPool",
        &json!({ "PoolName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateUserPool for {name:?}, got {status} with body {created}"
    );
    created
        .get("UserPool")
        .and_then(|pool| pool.get("Id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no UserPool.Id in {created}"))
        .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// One row of the table: a template (built around the pool a client already has), and what
/// the rule says the group it declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the pool and the group live in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), around the pool id
    /// the client's own pool carries.
    template: fn(&str) -> String,
    /// Extra `Parameters` the row's `CreateStack` carries, as `(key, value-is-the-pool-id)`.
    pool_parameter: Option<&'static str>,
    /// Further `Parameters` the row carries verbatim, as `(key, value)`. A `String`
    /// parameter is how a template feeds a number-valued property as TEXT: every value of
    /// one arrives as a string, whatever the owning API's member is modelled as.
    extra_parameters: &'static [(&'static str, &'static str)],
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// group carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `Description` the group must report, or `None` for a template declaring none.
    description: Option<&'static str>,
    /// The `Precedence` the group must report.
    precedence: Option<i64>,
    /// The `RoleArn` the group must report.
    role_arn: Option<&'static str>,
    /// The output keys whose value must be the group's name, each written with a different
    /// intrinsic over the same resource.
    name_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named group in JSON with every configuration property, read with Ref",
        region: "us-east-1",
        template: |pool| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a Cognito user pool group",
                "Resources": {
                    "Group": {
                        "Type": "AWS::Cognito::UserPoolGroup",
                        "Properties": {
                            "UserPoolId": pool,
                            "GroupName": "store-admins",
                            "Description": "the administrators of the store",
                            "Precedence": 1,
                            "RoleArn": ROLE_ARN
                        }
                    }
                },
                "Outputs": {
                    "GroupRef": { "Value": { "Ref": "Group" } },
                    "GroupSub": {
                        "Value": { "Fn::Sub": ["${Name}", { "Name": { "Ref": "Group" } }] }
                    }
                }
            })
            .to_string()
        },
        pool_parameter: None,
        extra_parameters: &[],
        declared_name: Some("store-admins"),
        description: Some("the administrators of the store"),
        precedence: Some(1),
        role_arn: Some(ROLE_ARN),
        name_outputs: &["GroupRef", "GroupSub"],
    },
    Row {
        case: "a named group in YAML, the pool and a String precedence through Parameters",
        region: "us-west-2",
        template: |_| {
            [
                "Parameters:",
                "  PoolId:",
                "    Type: String",
                "  Priority:",
                "    Type: String",
                "Resources:",
                "  Group:",
                "    Type: AWS::Cognito::UserPoolGroup",
                "    Properties:",
                "      UserPoolId: !Ref PoolId",
                "      GroupName: readers",
                "      Description: read-only members",
                "      Precedence: !Ref Priority",
                "Outputs:",
                "  GroupRef:",
                "    Value: !Ref Group",
                "",
            ]
            .join("\n")
        },
        pool_parameter: Some("PoolId"),
        extra_parameters: &[("Priority", "4")],
        declared_name: Some("readers"),
        description: Some("read-only members"),
        precedence: Some(4),
        role_arn: None,
        name_outputs: &["GroupRef"],
    },
    Row {
        case: "a group whose precedence is zero, the highest precedence value",
        region: "eu-west-1",
        template: |pool| {
            json!({
                "Resources": {
                    "Group": {
                        "Type": "AWS::Cognito::UserPoolGroup",
                        "Properties": {
                            "UserPoolId": pool,
                            "GroupName": "break-glass",
                            "Precedence": 0
                        }
                    }
                },
                "Outputs": { "GroupRef": { "Value": { "Ref": "Group" } } }
            })
            .to_string()
        },
        pool_parameter: None,
        extra_parameters: &[],
        declared_name: Some("break-glass"),
        description: None,
        precedence: Some(0),
        role_arn: None,
        name_outputs: &["GroupRef"],
    },
    Row {
        case: "a group the template does not name, with a quoted precedence scalar",
        region: "ap-northeast-1",
        template: |pool| {
            [
                "Resources:",
                "  Group:",
                "    Type: AWS::Cognito::UserPoolGroup",
                "    Properties:",
                &format!("      UserPoolId: {pool}"),
                "      Description: generated name",
                "      Precedence: \"9\"",
                "Outputs:",
                "  GroupRef:",
                "    Value: !Ref Group",
                "",
            ]
            .join("\n")
        },
        pool_parameter: None,
        extra_parameters: &[],
        declared_name: None,
        description: Some("generated name"),
        precedence: Some(9),
        role_arn: None,
        name_outputs: &["GroupRef"],
    },
];

/// Every row: the name the stack reports for its `AWS::Cognito::UserPoolGroup` is the
/// group's own, `cognito-idp GetGroup` resolves it to the group the template declared with
/// the configuration it declared, and deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_cognito_user_pool_group_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("cognito-group-stack-{index}");
        let pool = client_user_pool(&router, region, &format!("pool-{index}")).await;
        let template = (row.template)(&pool);

        // The `Parameters.member.<n>` pairs the row carries: the pool first where the row
        // feeds it through one, then its own.
        let mut parameters: Vec<(String, String)> = Vec::new();
        if let Some(key) = row.pool_parameter {
            parameters.push((key.to_string(), pool.clone()));
        }
        for (key, value) in row.extra_parameters {
            parameters.push(((*key).to_string(), (*value).to_string()));
        }
        let encoded: Vec<(String, String, String, String)> = parameters
            .iter()
            .enumerate()
            .map(|(position, (key, value))| {
                let member = position + 1;
                (
                    format!("Parameters.member.{member}.ParameterKey"),
                    key.clone(),
                    format!("Parameters.member.{member}.ParameterValue"),
                    value.clone(),
                )
            })
            .collect();
        let mut members: Vec<(&str, &str)> = vec![
            ("StackName", &stack_name),
            ("TemplateBody", &template),
        ];
        for (key_name, key, value_name, value) in &encoded {
            members.push((key_name, key));
            members.push((value_name, value));
        }
        let (status, body) = cloudformation(&router, region, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack reports the template it deployed once the create COMPLETES.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));
        // The resource is recorded under the type the template declared, beside the handle
        // the owning service holds for it.
        assert_eq!(
            opt_element(&resources, "ResourceType").as_deref(),
            Some("AWS::Cognito::UserPoolGroup"),
            "case {case:?}: expected the declared type to be reported, got {resources}"
        );

        // The physical resource id is the group NAME: the one the template asked for, or —
        // for a template that named nothing — one CloudFormation generated inside the bound
        // the owning model publishes for a group name.
        match row.declared_name {
            Some(declared) => assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared group name \
                 {declared:?}, got {physical_id:?}"
            ),
            None => {
                assert!(
                    physical_id.starts_with(&stack_name) && physical_id.contains("Group"),
                    "case {case:?}: expected a generated <stack>-<logical id>-<unique> name, got \
                     {physical_id:?}"
                );
                assert!(
                    !physical_id.is_empty() && physical_id.chars().count() <= MAX_GROUP_NAME,
                    "case {case:?}: expected a name inside the modelled 1..{MAX_GROUP_NAME}, got \
                     {physical_id:?}"
                );
                assert!(
                    physical_id.chars().all(|c| !c.is_whitespace()),
                    "case {case:?}: a group name admits no whitespace, got {physical_id:?}"
                );
            }
        }

        let outputs = outputs_of(&described);
        for key in row.name_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&physical_id),
                "case {case:?}: expected output {key:?} to be the group name {physical_id:?}, \
                 got {:?}",
                outputs.get(*key)
            );
        }

        // And Amazon Cognito itself resolves that pair to the group the template declared.
        let (status, read) = cognito(
            &router,
            region,
            "GetGroup",
            &json!({ "UserPoolId": &pool, "GroupName": &physical_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGroup on the name the stack reported, got \
             {status} with body {read}"
        );
        let group = read.get("Group").cloned().unwrap_or(Value::Null);
        assert_eq!(
            group.get("GroupName").and_then(Value::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the group to report the name {physical_id:?}, got {group}"
        );
        assert_eq!(
            group.get("UserPoolId").and_then(Value::as_str),
            Some(pool.as_str()),
            "case {case:?}: expected the group to be in the declared pool {pool:?}, got {group}"
        );
        assert_eq!(
            group.get("Description").and_then(Value::as_str),
            row.description,
            "case {case:?}: expected Description {:?}, got {group}",
            row.description
        );
        assert_eq!(
            group.get("Precedence").and_then(Value::as_i64),
            row.precedence,
            "case {case:?}: expected Precedence {:?}, got {group}",
            row.precedence
        );
        assert_eq!(
            group.get("RoleArn").and_then(Value::as_str),
            row.role_arn,
            "case {case:?}: expected RoleArn {:?}, got {group}",
            row.role_arn
        );

        // Deleting the stack deletes the group Amazon Cognito holds for it, and leaves the
        // client's own pool in place.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = cognito(
            &router,
            region,
            "GetGroup",
            &json!({ "UserPoolId": &pool, "GroupName": &physical_id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetGroup once the stack deleted the group, \
             got {status} with body {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException once the group was deleted, got \
             {gone}"
        );
        let (status, pool_still_there) = cognito(
            &router,
            region,
            "DescribeUserPool",
            &json!({ "UserPoolId": &pool }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the client's own pool to survive the stack's deletion, got \
             {status} with body {pool_still_there}"
        );
    }
}

/// A CHANGED declaration of a group the stack already has is applied through the owning
/// service's own `UpdateGroup`: the group keeps its name and its pool, and reports the
/// configuration the new template declares.
#[tokio::test]
async fn a_changed_group_declaration_is_applied_through_the_owning_service() {
    let router = build_router();
    let case = "an update that changes the description and the precedence";
    let region = "us-east-2";
    let stack_name = "cognito-group-update";
    let pool = client_user_pool(&router, region, "update-pool").await;

    let template = |description: &str, precedence: i64| {
        json!({
            "Resources": {
                "Group": {
                    "Type": "AWS::Cognito::UserPoolGroup",
                    "Properties": {
                        "UserPoolId": &pool,
                        "GroupName": "engineers",
                        "Description": description,
                        "Precedence": precedence
                    }
                }
            }
        })
        .to_string()
    };

    let created = template("the first description", 5);
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &created)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let updated = template("the second description", 2);
    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &updated)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the stack to reach UPDATE_COMPLETE, got {described}"
    );

    let (status, read) = cognito(
        &router,
        region,
        "GetGroup",
        &json!({ "UserPoolId": &pool, "GroupName": "engineers" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the group to still resolve, got {status} with body {read}"
    );
    let group = read.get("Group").cloned().unwrap_or(Value::Null);
    assert_eq!(
        group.get("Description").and_then(Value::as_str),
        Some("the second description"),
        "case {case:?}: expected the declared description to be in force, got {group}"
    );
    assert_eq!(
        group.get("Precedence").and_then(Value::as_i64),
        Some(2),
        "case {case:?}: expected the declared precedence to be in force, got {group}"
    );
}

/// A stack whose template declares a group Amazon Cognito REFUSES does not report a
/// resource nothing created: the stack fails with the refusal that service gave, and the
/// group a client already had is untouched.
///
/// The refusal here is Amazon Cognito's own `GroupExistsException` ("This name must be
/// unique in your user pool"), reached by pointing a template at a name a client already
/// created in the same pool.
#[tokio::test]
async fn a_group_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a template naming a group the pool already holds";
    let region = "eu-central-1";
    let pool = client_user_pool(&router, region, "clash-pool").await;

    let (status, created) = cognito(
        &router,
        region,
        "CreateGroup",
        &json!({
            "UserPoolId": &pool,
            "GroupName": "already-there",
            "Description": "the client's own group",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateGroup to succeed, got {status} with body \
         {created}"
    );

    let template = json!({
        "Resources": {
            "Group": {
                "Type": "AWS::Cognito::UserPoolGroup",
                "Properties": {
                    "UserPoolId": &pool,
                    "GroupName": "already-there",
                    "Description": "the stack's idea of it"
                }
            }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "group-clash"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "group-clash")]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Group"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    // The group the client created is still exactly as it was.
    let (status, read) = cognito(
        &router,
        region,
        "GetGroup",
        &json!({ "UserPoolId": &pool, "GroupName": "already-there" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's group to still resolve, got {status} with body \
         {read}"
    );
    assert_eq!(
        read.get("Group")
            .and_then(|group| group.get("Description"))
            .and_then(Value::as_str),
        Some("the client's own group"),
        "case {case:?}: expected the client's own description to be untouched, got {read}"
    );
}

/// A declaration that names no pool is refused by the provisioner itself, naming the
/// property the type requires, rather than sent as a request Amazon Cognito would reject:
/// `UserPoolId` is the other half of a group's identity.
#[tokio::test]
async fn a_group_declaring_no_user_pool_fails_the_stack() {
    let router = build_router();
    let case = "a group declaration with no UserPoolId";
    let region = "us-west-1";
    let template = json!({
        "Resources": {
            "Group": {
                "Type": "AWS::Cognito::UserPoolGroup",
                "Properties": { "GroupName": "poolless" }
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "group-no-pool"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "group-no-pool")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("UserPoolId"),
        "case {case:?}: expected the failure to name the missing property, got {reason:?}"
    );
}
