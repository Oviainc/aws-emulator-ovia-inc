// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Cognito::IdentityPool` really creates the
//! pool in Amazon Cognito Identity, and the handle the stack hands back is the one that
//! service answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the stack
//! publishes for it is the one that type's published return values document — for an identity
//! pool, "Ref returns the Amazon Cognito identity pool ID", the `REGION:GUID` value whose
//! modelled `@pattern ^[\w-]+:[0-9a-f-]+$` every `IdentityPoolId` member carries — so a
//! stack output wired to `!Ref` can be passed straight to `GetId`, `DescribeIdentityPool` and
//! `GetIdentityPoolRoles`.** Two crates have to agree for that to hold (CloudFormation makes
//! the call, Amazon Cognito Identity serves it), so the suite lives here rather than in
//! either crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the pool
//! or leaves CloudFormation to generate its name, whether unauthenticated identities are
//! admitted, which provider configuration and tags the pool declares, whether the stack also
//! declares the `AWS::Cognito::IdentityPoolRoleAttachment` that binds its roles, whether the
//! template is written in JSON or YAML, which intrinsic an output reads the resource with
//! (`Ref`, `Fn::Sub`), and which Region the stack deploys in.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the pool ID itself, which is the owning service's to
//! mint: what is checked about it is that it satisfies the modelled pattern, that it names
//! the Region the stack deployed in, and that the owning service resolves it.

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

/// The emulator with the services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: CloudFormation, the Amazon Cognito Identity service
/// that owns the pool, and IAM, which holds the roles a role attachment names.
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
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
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
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon Cognito Identity operation over the awsJson1_1 wire its model declares,
/// signed for `region`.
async fn cognito_identity(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-identity.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityService.{operation}"),
        )
        .header("authorization", authorization("cognito-identity", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Seed one IAM role a role attachment can name, with a trust policy admitting the Cognito
/// Identity principal, and answer its ARN.
async fn iam_role(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": "cognito-identity.amazonaws.com"},
            "Action": "sts:AssumeRoleWithWebIdentity",
        }],
    })
    .to_string();
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(name),
        form_encode(&trust)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "expected HTTP 200 from IAM CreateRole for {name:?}, got {status} with body {payload}"
    );
    opt_element(&payload, "Arn")
        .unwrap_or_else(|| panic!("no <Arn> in the created role: {payload}"))
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Whether `id` is an `IdentityPoolId` of `region`: the `REGION:GUID` form the model's
/// `@pattern ^[\w-]+:[0-9a-f-]+$` admits, which is what every `IdentityPoolId` member takes.
fn is_identity_pool_id(id: &str, region: &str) -> bool {
    let Some((named_region, guid)) = id.split_once(':') else {
        return false;
    };
    named_region == region
        && !guid.is_empty()
        && guid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

/// One row of the table: the template it deploys, and what the rule says the pool it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the pool lives in and the one its
    /// own id names.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), around the ARN of the
    /// role the row's attachment names.
    template: fn(&str) -> String,
    /// The `IdentityPoolName` the template declares, or `None` for a template that names
    /// nothing — whose pool carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// What the pool must report for `AllowUnauthenticatedIdentities`.
    allow_unauthenticated: bool,
    /// The provider configuration the pool must report, as `(member, value)` pairs.
    reported_members: fn() -> Vec<(&'static str, Value)>,
    /// Whether the stack also declares the role attachment that binds the pool's roles.
    attaches_roles: bool,
    /// The output keys whose value must be the pool's id, each written with a different
    /// intrinsic over the same resource.
    id_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named pool in JSON with a role attachment, read with Ref and Fn::Sub",
        region: "us-east-1",
        template: |role| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an identity pool and its roles",
                "Resources": {
                    "IdentityPool": {
                        "Type": "AWS::Cognito::IdentityPool",
                        "Properties": {
                            "IdentityPoolName": "deployed-pool",
                            "AllowUnauthenticatedIdentities": true,
                            "SupportedLoginProviders": {"graph.facebook.com": "an-app-id"},
                            "IdentityPoolTags": {"Owner": "identity"}
                        }
                    },
                    "Roles": {
                        "Type": "AWS::Cognito::IdentityPoolRoleAttachment",
                        "Properties": {
                            "IdentityPoolId": {"Ref": "IdentityPool"},
                            "Roles": {"unauthenticated": role, "authenticated": role}
                        }
                    }
                },
                "Outputs": {
                    "PoolRef": {"Value": {"Ref": "IdentityPool"}},
                    "PoolSub": {
                        "Value": {"Fn::Sub": ["${Id}", {"Id": {"Ref": "IdentityPool"}}]}
                    }
                }
            })
            .to_string()
        },
        declared_name: Some("deployed-pool"),
        allow_unauthenticated: true,
        reported_members: || {
            vec![
                (
                    "SupportedLoginProviders",
                    json!({"graph.facebook.com": "an-app-id"}),
                ),
                ("IdentityPoolTags", json!({"Owner": "identity"})),
            ]
        },
        attaches_roles: true,
        id_outputs: &["PoolRef", "PoolSub"],
    },
    Row {
        case: "a YAML pool that admits no unauthenticated identity, with a user-pool provider",
        region: "us-west-2",
        template: |_| {
            [
                "Resources:",
                "  IdentityPool:",
                "    Type: AWS::Cognito::IdentityPool",
                "    Properties:",
                "      IdentityPoolName: deployed-authenticated-pool",
                "      AllowUnauthenticatedIdentities: false",
                "      DeveloperProviderName: login.deployed.developer",
                "      CognitoIdentityProviders:",
                "        - ProviderName: cognito-idp.us-west-2.amazonaws.com/us-west-2_deployed1",
                "          ClientId: deployedclient",
                "Outputs:",
                "  PoolRef:",
                "    Value: !Ref IdentityPool",
                "",
            ]
            .join("\n")
        },
        declared_name: Some("deployed-authenticated-pool"),
        allow_unauthenticated: false,
        reported_members: || {
            vec![
                (
                    "DeveloperProviderName",
                    json!("login.deployed.developer"),
                ),
                (
                    "CognitoIdentityProviders",
                    json!([{
                        "ProviderName": "cognito-idp.us-west-2.amazonaws.com/us-west-2_deployed1",
                        "ClientId": "deployedclient",
                    }]),
                ),
            ]
        },
        attaches_roles: false,
        id_outputs: &["PoolRef"],
    },
    Row {
        case: "a pool the template does not name, in a third Region",
        region: "eu-west-1",
        template: |_| {
            json!({
                "Resources": {
                    "IdentityPool": {
                        "Type": "AWS::Cognito::IdentityPool",
                        "Properties": {"AllowUnauthenticatedIdentities": true}
                    }
                },
                "Outputs": {"PoolRef": {"Value": {"Ref": "IdentityPool"}}}
            })
            .to_string()
        },
        declared_name: None,
        allow_unauthenticated: true,
        reported_members: Vec::new,
        attaches_roles: false,
        id_outputs: &["PoolRef"],
    },
];

/// Every row: the id the stack reports for its `AWS::Cognito::IdentityPool` is the pool id
/// the owning service minted, every output wired to the resource carries it, that service
/// resolves it to the pool the template declared, `GetId` mints an identity in it, and
/// deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_publishes_the_identity_pool_id_the_owning_service_minted() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("identity-pool-stack-{index}");
        let role = iam_role(&router, region, &format!("deployed-pool-role-{index}")).await;
        let template = (row.template)(&role);

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)])
                .await;
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

        // The id every output wired to the resource carries, which is the one the rule is
        // about: a pool id of the Region the stack deployed in.
        let outputs = outputs_of(&described);
        let pool_id = outputs
            .get(row.id_outputs[0])
            .unwrap_or_else(|| panic!("case {case:?}: no {:?} output in {outputs:?}", row.id_outputs[0]))
            .clone();
        assert!(
            is_identity_pool_id(&pool_id, region),
            "case {case:?}: expected a REGION:GUID identity pool id of {region}, got {pool_id:?}"
        );
        for key in row.id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&pool_id),
                "case {case:?}: expected output {key:?} to be the pool id {pool_id:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And the stack records that same id as the resource's physical id, under the type
        // the template declared.
        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name), ("LogicalResourceId", "IdentityPool")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        assert_eq!(
            opt_element(&resources, "PhysicalResourceId").as_deref(),
            Some(pool_id.as_str()),
            "case {case:?}: expected the pool id as the physical resource id, got {resources}"
        );
        assert_eq!(
            opt_element(&resources, "ResourceType").as_deref(),
            Some("AWS::Cognito::IdentityPool"),
            "case {case:?}: expected the declared type to be reported, got {resources}"
        );

        // Amazon Cognito Identity itself resolves that id to the pool the template declared.
        let (status, pool) = cognito_identity(
            &router,
            region,
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeIdentityPool on the id the stack \
             published, got {status} with body {pool}"
        );
        match row.declared_name {
            Some(declared) => assert_eq!(
                pool.get("IdentityPoolName").and_then(Value::as_str),
                Some(declared),
                "case {case:?}: expected the declared pool name {declared:?}, got {pool}"
            ),
            None => {
                let generated = pool
                    .get("IdentityPoolName")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    generated.starts_with(&stack_name) && generated.contains("IdentityPool"),
                    "case {case:?}: expected a generated <stack>-<logical id>-<unique> name, \
                     got {generated:?}"
                );
            }
        }
        assert_eq!(
            pool.get("AllowUnauthenticatedIdentities")
                .and_then(Value::as_bool),
            Some(row.allow_unauthenticated),
            "case {case:?}: expected AllowUnauthenticatedIdentities {}, got {pool}",
            row.allow_unauthenticated
        );
        for (member, expected) in (row.reported_members)() {
            if member == "IdentityPoolTags" {
                // CloudFormation propagates its own three tags beside the declared one, so
                // what is checked is that the declared pair is among what the pool reports.
                let reported = pool.get(member).cloned().unwrap_or(Value::Null);
                for (key, value) in expected.as_object().into_iter().flatten() {
                    assert_eq!(
                        reported.get(key),
                        Some(value),
                        "case {case:?}: expected tag {key:?} to be {value}, got {reported}"
                    );
                }
                continue;
            }
            assert_eq!(
                pool.get(member),
                Some(&expected),
                "case {case:?}: expected {member} to be {expected}, got {pool}"
            );
        }

        // The id a stack publishes is one a client can CALL that service with, which is the
        // whole point of the rule: an unauthenticated `GetId` mints an identity in a pool
        // that admits one, and is refused for the modelled reason in a pool that does not.
        let (status, minted) = cognito_identity(
            &router,
            region,
            "GetId",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
        if row.allow_unauthenticated {
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetId to mint an identity in the pool the stack \
                 deployed, got {status} with body {minted}"
            );
            let identity_id = minted
                .get("IdentityId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                is_identity_pool_id(identity_id, region),
                "case {case:?}: expected a minted identity of {region}, got {identity_id:?}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    minted.get("__type").and_then(Value::as_str).unwrap_or_default()
                ),
                (400, "NotAuthorizedException"),
                "case {case:?}: expected the unauthenticated call to be refused, got {status} \
                 with body {minted}"
            );
        }

        // The roles a role attachment declared are the ones that pool reports.
        let (status, roles) = cognito_identity(
            &router,
            region,
            "GetIdentityPoolRoles",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetIdentityPoolRoles, got {status} with \
             body {roles}"
        );
        let reported_roles = roles.get("Roles").cloned().unwrap_or(Value::Null);
        if row.attaches_roles {
            assert_eq!(
                reported_roles,
                json!({"authenticated": &role, "unauthenticated": &role}),
                "case {case:?}: expected the attached roles to be in force, got {roles}"
            );
        } else {
            assert!(
                reported_roles.is_null(),
                "case {case:?}: a stack declaring no attachment configures no role, got {roles}"
            );
        }

        // Deleting the stack deletes the pool in the service that owns it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = cognito_identity(
            &router,
            region,
            "DescribeIdentityPool",
            &json!({"IdentityPoolId": &pool_id}),
        )
        .await;
        assert_eq!(
            (
                status,
                gone.get("__type").and_then(Value::as_str).unwrap_or_default()
            ),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the pool to be gone with the stack, got {status} with \
             body {gone}"
        );
    }
}

/// RULE, applied to the mapping half of the attachment: a `RoleMappings` entry a template
/// declares reaches the owning service keyed the way `SetIdentityPoolRoles` keys one — by the
/// identity provider the entry NAMES, whichever key the template held it under — so the pool
/// the stack deployed reports the mapping a client would have set itself.
#[tokio::test]
async fn a_stack_applies_the_role_mappings_its_attachment_declares() {
    let router = build_router();
    let region = "us-east-1";
    let stack_name = "identity-pool-role-mapping-stack";
    let case = "a Token mapping held under a key of the template's own";
    // The provider the mapping is FOR: a user pool and the app client it issues tokens to,
    // which is the identifier a role mapping of such a provider is keyed by.
    let provider = "cognito-idp.us-east-1.amazonaws.com/us-east-1_mapped1:mappedclient";

    let template = json!({
        "Resources": {
            "IdentityPool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": "role-mapped-pool",
                    // A quoted scalar, which is how a template feeds a boolean property
                    // through a `String` parameter.
                    "AllowUnauthenticatedIdentities": "false",
                    "CognitoIdentityProviders": [{
                        "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_mapped1",
                        "ClientId": "mappedclient",
                        "ServerSideTokenCheck": false,
                    }],
                }
            },
            "AuthRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "role-mapped-pool-authenticated",
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Federated": "cognito-identity.amazonaws.com"},
                            "Action": "sts:AssumeRoleWithWebIdentity",
                        }],
                    },
                }
            },
            "RoleAttachment": {
                "Type": "AWS::Cognito::IdentityPoolRoleAttachment",
                "Properties": {
                    "IdentityPoolId": {"Ref": "IdentityPool"},
                    "Roles": {"authenticated": {"Fn::GetAtt": ["AuthRole", "Arn"]}},
                    "RoleMappings": {"userpool": {
                        "IdentityProvider": provider,
                        "Type": "Token",
                        "AmbiguousRoleResolution": "AuthenticatedRole",
                    }},
                }
            }
        },
        "Outputs": {"PoolRef": {"Value": {"Ref": "IdentityPool"}}}
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    // A refused resource would have rolled the stack back, so the status is what says the
    // owning service accepted every call the stack made.
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected CREATE_COMPLETE, got {described} with events {events}"
    );

    let pool_id = outputs_of(&described)
        .get("PoolRef")
        .unwrap_or_else(|| panic!("case {case:?}: no PoolRef output in {described}"))
        .clone();
    let (status, roles) = cognito_identity(
        &router,
        region,
        "GetIdentityPoolRoles",
        &json!({"IdentityPoolId": &pool_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetIdentityPoolRoles, got {status} with body \
         {roles}"
    );
    let mappings = roles.get("RoleMappings").cloned().unwrap_or(Value::Null);
    let keys: Vec<&String> = mappings
        .as_object()
        .map(|mappings| mappings.keys().collect())
        .unwrap_or_default();
    assert_eq!(
        keys,
        vec![&provider.to_string()],
        "case {case:?}: expected the mapping to be keyed by the provider it names, got {roles}"
    );
    let entry = mappings.get(provider).cloned().unwrap_or(Value::Null);
    assert_eq!(
        entry.get("Type"),
        Some(&json!("Token")),
        "case {case:?}: expected the declared mapping type to be in force, got {roles}"
    );
    assert_eq!(
        entry.get("AmbiguousRoleResolution"),
        Some(&json!("AuthenticatedRole")),
        "case {case:?}: expected the declared resolution to be in force, got {roles}"
    );

    // The pool's own flag was declared as a quoted scalar, and the boolean the API models is
    // what the pool reports.
    let (_, pool) = cognito_identity(
        &router,
        region,
        "DescribeIdentityPool",
        &json!({"IdentityPoolId": &pool_id}),
    )
    .await;
    assert_eq!(
        pool.get("AllowUnauthenticatedIdentities")
            .and_then(Value::as_bool),
        Some(false),
        "case {case:?}: expected the quoted flag to reach the pool as a boolean, got {pool}"
    );
}

/// RULE, on the second deployment: a stack UPDATE that changes the pool's declaration changes
/// the pool the stack already has — the identifier the owning service minted stays in force,
/// so the id a client holds keeps resolving — and what that pool then does at runtime follows
/// the new configuration.
#[tokio::test]
async fn a_stack_update_changes_the_pool_it_already_created() {
    let router = build_router();
    let region = "us-east-1";
    let stack_name = "identity-pool-update-stack";
    let case = "an update that stops admitting unauthenticated identities";

    let template = |allow: bool| {
        json!({
            "Resources": {
                "IdentityPool": {
                    "Type": "AWS::Cognito::IdentityPool",
                    "Properties": {
                        "IdentityPoolName": "updated-pool",
                        "AllowUnauthenticatedIdentities": allow,
                    }
                }
            },
            "Outputs": {"PoolRef": {"Value": {"Ref": "IdentityPool"}}}
        })
        .to_string()
    };

    let created = template(true);
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
    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    let pool_id = outputs_of(&described)
        .get("PoolRef")
        .unwrap_or_else(|| panic!("case {case:?}: no PoolRef output in {described}"))
        .clone();
    assert!(
        is_identity_pool_id(&pool_id, region),
        "case {case:?}: expected a REGION:GUID pool id, got {pool_id:?}"
    );

    // Before the update the pool admits an unauthenticated identity.
    let (status, minted) = cognito_identity(
        &router,
        region,
        "GetId",
        &json!({"IdentityPoolId": &pool_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the pool to admit an unauthenticated identity before the \
         update, got {status} with body {minted}"
    );

    let updated = template(false);
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

    // The pool keeps the identity its create minted: the update changed the pool the stack
    // already had rather than replacing it with another one.
    assert_eq!(
        outputs_of(&described).get("PoolRef"),
        Some(&pool_id),
        "case {case:?}: expected the same pool id after the update, got {described}"
    );
    let (status, pool) = cognito_identity(
        &router,
        region,
        "DescribeIdentityPool",
        &json!({"IdentityPoolId": &pool_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the pool to still resolve after the update, got {status} with \
         body {pool}"
    );
    assert_eq!(
        pool.get("AllowUnauthenticatedIdentities")
            .and_then(Value::as_bool),
        Some(false),
        "case {case:?}: expected the updated configuration to be in force, got {pool}"
    );

    // ...and what the pool DOES follows it: the call that was served before the update is
    // refused after it.
    let (status, refused) = cognito_identity(
        &router,
        region,
        "GetId",
        &json!({"IdentityPoolId": &pool_id}),
    )
    .await;
    assert_eq!(
        (
            status,
            refused.get("__type").and_then(Value::as_str).unwrap_or_default()
        ),
        (400, "NotAuthorizedException"),
        "case {case:?}: expected the unauthenticated call to be refused after the update, got \
         {status} with body {refused}"
    );

    // ...and an update that INTRODUCES a property this emulator does not apply is refused
    // rather than reported as applied: a property of another Amazon Cognito API reaches the
    // deployment only in the new declaration, so the update path has to judge that document
    // and not the one the resource was created with.
    let case = "an update introducing a property of another Amazon Cognito API";
    let unapplied = json!({
        "Resources": {
            "IdentityPool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": "updated-pool",
                    "AllowUnauthenticatedIdentities": false,
                    // `PushSync` is a cognito-sync configuration, which this emulator does
                    // not apply.
                    "PushSync": {"ApplicationArns": []},
                }
            }
        },
        "Outputs": {"PoolRef": {"Value": {"Ref": "IdentityPool"}}}
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &unapplied)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    let status_after = opt_element(&described, "StackStatus").unwrap_or_default();
    assert!(
        status_after.starts_with("UPDATE_ROLLBACK") || status_after == "UPDATE_FAILED",
        "case {case:?}: expected the update to fail rather than report a change it never \
         applied, got {status_after:?} in {described}"
    );
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    assert!(
        events.contains("PushSync"),
        "case {case:?}: expected the failure to name the property this emulator does not \
         apply, got {events}"
    );
}
