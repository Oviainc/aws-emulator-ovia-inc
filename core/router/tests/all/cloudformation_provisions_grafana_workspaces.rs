// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::Grafana::Workspace` resources its template
//! declares — in Amazon Managed Grafana — and the handle it publishes for each one is the
//! workspace id that service minted.
//!
//! The rule under test: **a template may declare any resource type AWS publishes, and a
//! stack reaches its completed status only once the service that owns each declared
//! resource has created it — with the physical id the stack reports (`Ref`,
//! `DescribeStackResources`) being the id the OWNING service answered with, wherever that
//! service is the one that mints the resource's identity.** `AWS::Grafana::Workspace`'s
//! published `Ref` is the workspace id, a `^g-[0-9a-f]{10}$` value only Amazon Managed
//! Grafana produces, so the stack's output has to be an id `DescribeWorkspace` resolves —
//! not the `Name` the template asked for, and not a `ValidationError` refusing the template
//! at submission. Two crates have to agree for that to hold (CloudFormation makes the
//! calls, Amazon Managed Grafana serves them), so the suite lives here: a crate-local test
//! can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! workspace or leaves the name to CloudFormation, which of the type's properties it
//! declares (the required three, a description, a Grafana version, the data-source and
//! notification lists, the nested `VpcConfiguration` and `NetworkAccessControl`), how many
//! workspaces one stack declares, how the output is written (`Ref`, `Fn::GetAtt Id`), and
//! which spelling the template is in (JSON and YAML). Every expected value is computed from
//! what the row declares, never read back out of the answer being checked.
//!
//! The other halves of the same rule are tests of their own below: a direct
//! `UpdateWorkspace` on a stack-created workspace still persists (the control plane is not
//! stack-exclusive), deleting the stack deletes the workspace — so `DescribeWorkspace` and
//! `UpdateWorkspace` then answer `ResourceNotFoundException` — and a declaration carrying a
//! property `CreateWorkspace` cannot take leaves the stack `CREATE_FAILED` naming the
//! resource, with nothing created in that service.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery wire helpers this test crate publishes ONCE, in `query_wire`.
use crate::query_wire::{elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack and its
/// workspaces belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (500 ms) and for the workspace's own
/// `CREATING` -> `ACTIVE` transition (500 ms) to come due.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_500);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The IAM role every workspace below views data through. Any ARN serves: Amazon Managed
/// Grafana stores `workspaceRoleArn` as it is given.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/grafana-workspace";

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
        service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon Managed Grafana operation over its own restJson1 wire.
async fn grafana(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("grafana.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("grafana"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a restJson1 refusal, as a client reads it off the body's
/// `__type`.
fn grafana_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Create a stack, wait past the transitions, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack for a published resource type, \
         got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_SPANS).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Whether `id` matches `WorkspaceId`'s published pattern (`^g-[0-9a-f]{10}$`) — the form
/// of an id Amazon Managed Grafana minted, which is what distinguishes it from a name the
/// template or CloudFormation chose. Checked here rather than through the owning crate's
/// own predicate, so a wrong predicate cannot make this pass.
fn is_workspace_id(id: &str) -> bool {
    id.strip_prefix("g-").is_some_and(|hex| {
        hex.len() == 10
            && hex
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// What the row says about one workspace the template declares.
struct DeclaredWorkspace {
    /// The resource's logical id, which a generated name is built from.
    logical_id: &'static str,
    /// The output key carrying `{"Ref": <logical id>}`.
    ref_output: &'static str,
    /// Output keys that are `Fn::GetAtt` of this resource and must carry the same id the
    /// `Ref` does: the uniform `Id`/`<Type>Id` return values.
    id_outputs: &'static [&'static str],
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// The members `DescribeWorkspace` must report for it, as (JSON pointer into the
    /// reported `workspace`, expected value) pairs computed from the declaration.
    reported: &'static [(&'static str, &'static str)],
}

/// One row: a template and the workspaces it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    workspaces: &'static [DeclaredWorkspace],
}

const ROWS: &[Row] = &[
    Row {
        // The reported case: one named workspace carrying a description, with the id read
        // back through `Ref`.
        case: "a named workspace with a description, output by Ref",
        stack_name: "grafana-cfn-stack",
        template: r#"{"AWSTemplateFormatVersion":"2010-09-09",
             "Resources":{"GrafanaWorkspace":{"Type":"AWS::Grafana::Workspace",
               "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                 "AuthenticationProviders":["SAML"],
                 "PermissionType":"SERVICE_MANAGED",
                 "RoleArn":"arn:aws:iam::000000000000:role/grafana-workspace",
                 "Name":"behav-cfn-workspace",
                 "Description":"behav-cfn-initial-description"}}},
             "Outputs":{"WorkspaceId":{"Value":{"Ref":"GrafanaWorkspace"}}}}"#,
        workspaces: &[DeclaredWorkspace {
            logical_id: "GrafanaWorkspace",
            ref_output: "WorkspaceId",
            id_outputs: &[],
            declared_name: Some("behav-cfn-workspace"),
            reported: &[
                ("/name", "behav-cfn-workspace"),
                ("/description", "behav-cfn-initial-description"),
                ("/accountAccessType", "CURRENT_ACCOUNT"),
                ("/permissionType", "SERVICE_MANAGED"),
                ("/authentication/providers/0", "SAML"),
                ("/workspaceRoleArn", ROLE_ARN),
                ("/status", "ACTIVE"),
            ],
        }],
    },
    Row {
        case: "an unnamed workspace with a version and the two nested configurations, \
               output by Ref and Fn::GetAtt Id",
        stack_name: "grafana-unnamed-stack",
        template: r#"{"Resources":{"Monitoring":{"Type":"AWS::Grafana::Workspace",
               "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                 "AuthenticationProviders":["AWS_SSO"],
                 "PermissionType":"CUSTOMER_MANAGED",
                 "GrafanaVersion":"9.4",
                 "DataSources":["CLOUDWATCH","PROMETHEUS"],
                 "NotificationDestinations":["SNS"],
                 "VpcConfiguration":{"SecurityGroupIds":["sg-0123456789abcdef0"],
                   "SubnetIds":["subnet-0123456789abcdef0","subnet-0fedcba9876543210"]},
                 "NetworkAccessControl":{"PrefixListIds":["pl-0123456789abcdef0"],
                   "VpceIds":["vpce-0123456789abcdef0"]}}}},
             "Outputs":{"MonitoringId":{"Value":{"Ref":"Monitoring"}},
               "MonitoringAttId":{"Value":{"Fn::GetAtt":["Monitoring","Id"]}}}}"#,
        workspaces: &[DeclaredWorkspace {
            logical_id: "Monitoring",
            ref_output: "MonitoringId",
            id_outputs: &["MonitoringAttId"],
            declared_name: None,
            reported: &[
                ("/accountAccessType", "CURRENT_ACCOUNT"),
                ("/permissionType", "CUSTOMER_MANAGED"),
                ("/authentication/providers/0", "AWS_SSO"),
                ("/grafanaVersion", "9.4"),
                ("/dataSources/0", "CLOUDWATCH"),
                ("/dataSources/1", "PROMETHEUS"),
                ("/notificationDestinations/0", "SNS"),
                ("/vpcConfiguration/securityGroupIds/0", "sg-0123456789abcdef0"),
                ("/vpcConfiguration/subnetIds/0", "subnet-0123456789abcdef0"),
                ("/vpcConfiguration/subnetIds/1", "subnet-0fedcba9876543210"),
                ("/networkAccessControl/prefixListIds/0", "pl-0123456789abcdef0"),
                ("/networkAccessControl/vpceIds/0", "vpce-0123456789abcdef0"),
                ("/status", "ACTIVE"),
            ],
        }],
    },
    Row {
        case: "two workspaces in one stack, one named and one not",
        stack_name: "grafana-pair-stack",
        template: r#"{"Resources":{
               "Primary":{"Type":"AWS::Grafana::Workspace",
                 "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                   "AuthenticationProviders":["SAML"],
                   "PermissionType":"SERVICE_MANAGED",
                   "Name":"primary-observability",
                   "Description":"the primary workspace"}},
               "Secondary":{"Type":"AWS::Grafana::Workspace",
                 "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                   "AuthenticationProviders":["SAML","AWS_SSO"],
                   "PermissionType":"CUSTOMER_MANAGED"}}},
             "Outputs":{"PrimaryId":{"Value":{"Ref":"Primary"}},
               "SecondaryId":{"Value":{"Ref":"Secondary"}}}}"#,
        workspaces: &[
            DeclaredWorkspace {
                logical_id: "Primary",
                ref_output: "PrimaryId",
                id_outputs: &[],
                declared_name: Some("primary-observability"),
                reported: &[
                    ("/name", "primary-observability"),
                    ("/description", "the primary workspace"),
                    ("/status", "ACTIVE"),
                ],
            },
            DeclaredWorkspace {
                logical_id: "Secondary",
                ref_output: "SecondaryId",
                id_outputs: &[],
                declared_name: None,
                reported: &[
                    ("/authentication/providers/0", "SAML"),
                    ("/authentication/providers/1", "AWS_SSO"),
                    ("/permissionType", "CUSTOMER_MANAGED"),
                    ("/status", "ACTIVE"),
                ],
            },
        ],
    },
    Row {
        case: "the YAML spelling of a template declaring a named workspace",
        stack_name: "grafana-yaml-stack",
        template: concat!(
            "Resources:\n",
            "  Dashboards:\n",
            "    Type: AWS::Grafana::Workspace\n",
            "    Properties:\n",
            "      AccountAccessType: CURRENT_ACCOUNT\n",
            "      AuthenticationProviders:\n",
            "        - SAML\n",
            "      PermissionType: SERVICE_MANAGED\n",
            "      RoleArn: arn:aws:iam::000000000000:role/grafana-workspace\n",
            "      Name: yaml-dashboards\n",
            "      Description: declared in YAML\n",
            "Outputs:\n",
            "  DashboardsId:\n",
            "    Value: !Ref Dashboards\n",
        ),
        workspaces: &[DeclaredWorkspace {
            logical_id: "Dashboards",
            ref_output: "DashboardsId",
            id_outputs: &[],
            declared_name: Some("yaml-dashboards"),
            reported: &[
                ("/name", "yaml-dashboards"),
                ("/description", "declared in YAML"),
                ("/workspaceRoleArn", ROLE_ARN),
                ("/status", "ACTIVE"),
            ],
        }],
    },
];

/// The `PhysicalResourceId` and `ResourceStatus` a stack reports for one logical resource.
async fn stack_resource(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    logical_id: &str,
) -> (String, String) {
    let (status, body) = cloudformation(
        router,
        "DescribeStackResources",
        &[
            ("StackName", stack_name),
            ("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
         body {body}"
    );
    (
        opt_element(&body, "PhysicalResourceId").unwrap_or_default(),
        opt_element(&body, "ResourceStatus").unwrap_or_default(),
    )
}

#[tokio::test]
async fn a_completed_stack_has_created_every_workspace_it_declares_in_grafana() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for workspace in row.workspaces {
            let case = format!("{} [{}]", row.case, workspace.logical_id);
            let id = outputs
                .get(workspace.ref_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", workspace.ref_output)
                })
                .clone();

            // The Ref is an id Amazon Managed Grafana minted, not the name the template
            // asked for.
            assert!(
                is_workspace_id(&id),
                "case {case:?}: expected the Ref to be a minted workspace id matching \
                 ^g-[0-9a-f]{{10}}$, got {id:?}"
            );
            if let Some(declared) = workspace.declared_name {
                assert_ne!(
                    id, declared,
                    "case {case:?}: expected the Ref to be the minted workspace id rather \
                     than the declared Name"
                );
            }
            for output in workspace.id_outputs {
                let reported = outputs.get(*output);
                assert_eq!(
                    reported.map(String::as_str),
                    Some(id.as_str()),
                    "case {case:?}: expected the {output} output to be the resource's id \
                     {id:?}, got {reported:?}"
                );
            }

            // The stack's own per-resource read reports that same id, as created.
            let (physical_id, resource_status) =
                stack_resource(&router, &case, row.stack_name, workspace.logical_id).await;
            assert_eq!(
                physical_id, id,
                "case {case:?}: expected the PhysicalResourceId to be the minted id {id:?}, \
                 got {physical_id:?}"
            );
            assert_eq!(
                resource_status, "CREATE_COMPLETE",
                "case {case:?}: expected the resource to be CREATE_COMPLETE, got \
                 {resource_status:?}"
            );

            // Amazon Managed Grafana itself resolves that id, and what it holds is what
            // the template declared.
            let (status, described) =
                grafana(&router, "GET", &format!("/workspaces/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeWorkspace to resolve the published id \
                 {id:?}, got {status} with {described}"
            );
            let reported_workspace = described
                .get("workspace")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                reported_workspace.pointer("/id"),
                Some(&json!(id)),
                "case {case:?}: expected the workspace to report the id it was read by, got \
                 {reported_workspace}"
            );
            for (pointer, expected) in workspace.reported {
                let found = reported_workspace.pointer(pointer).cloned();
                assert_eq!(
                    found,
                    Some(json!(expected)),
                    "case {case:?}: expected {pointer} to be {expected:?}, got {found:?} in \
                     {reported_workspace}"
                );
            }
            if workspace.declared_name.is_none() {
                let name = reported_workspace
                    .pointer("/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    is_generated_name(name, row.stack_name, workspace.logical_id),
                    "case {case:?}: expected the generated \
                     <stack>-<logical id>-<suffix> name, got {name:?}"
                );
            }
        }
    }
}

/// The control plane is not stack-exclusive: a direct `UpdateWorkspace` on a workspace a
/// stack created still persists, and deleting the stack deletes that workspace — so the
/// id the stack published then resolves nowhere.
#[tokio::test]
async fn a_direct_update_persists_and_deleting_the_stack_deletes_the_workspace() {
    let case = "the stack-created workspace, updated directly and then torn down";
    let stack_name = "grafana-teardown-stack";
    let template = r#"{"Resources":{"GrafanaWorkspace":{"Type":"AWS::Grafana::Workspace",
           "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
             "AuthenticationProviders":["SAML"],
             "PermissionType":"SERVICE_MANAGED",
             "RoleArn":"arn:aws:iam::000000000000:role/grafana-workspace",
             "Description":"behav-cfn-initial-description"}}},
         "Outputs":{"WorkspaceId":{"Value":{"Ref":"GrafanaWorkspace"}}}}"#;

    let router = build_router();
    let body = create_and_settle(&router, case, stack_name, template).await;
    let id = outputs_of(&body)
        .get("WorkspaceId")
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceId output in {body}"))
        .clone();
    assert!(
        is_workspace_id(&id),
        "case {case:?}: expected a minted workspace id, got {id:?}"
    );

    // A direct update of the workspace the stack created still persists.
    let (status, updated) = grafana(
        &router,
        "PUT",
        &format!("/workspaces/{id}"),
        Some(json!({"workspaceDescription": "behav-direct-update"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected UpdateWorkspace to be accepted, got {status} with {updated}"
    );
    let (status, described) = grafana(&router, "GET", &format!("/workspaces/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeWorkspace to answer, got {status} with {described}"
    );
    assert_eq!(
        described.pointer("/workspace/description"),
        Some(&json!("behav-direct-update")),
        "case {case:?}: expected the direct update to persist, got {described}"
    );

    // Deleting the stack deletes the workspace it created.
    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_SPANS).await;

    let (status, described) = grafana(&router, "GET", &format!("/workspaces/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected DescribeWorkspace to answer 404 for the torn-down \
         workspace, got {status} with {described}"
    );
    assert_eq!(
        grafana_error_code(&described),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {described}"
    );
    let (status, refused) = grafana(
        &router,
        "PUT",
        &format!("/workspaces/{id}"),
        Some(json!({"workspaceDescription": "after the teardown"})),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected UpdateWorkspace to answer 404 for the torn-down \
         workspace, got {status} with {refused}"
    );
    assert_eq!(
        grafana_error_code(&refused),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {refused}"
    );
}

/// A declaration carrying a property `CreateWorkspace` cannot take is not created
/// differently from what it declares: the stack fails naming the resource, and Amazon
/// Managed Grafana holds no workspace.
#[tokio::test]
async fn a_property_the_create_cannot_take_leaves_the_stack_failed() {
    // Each row declares one property of the type that no `CreateWorkspace` member carries:
    // a workspace's SAML configuration is written by `UpdateWorkspaceAuthentication` and
    // its plugin administration by `UpdateWorkspaceConfiguration`.
    let rows: [(&str, &str, &str); 2] = [
        (
            "a declared SamlConfiguration",
            "grafana-saml-stack",
            r#"{"Resources":{"Workspace":{"Type":"AWS::Grafana::Workspace",
                 "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                   "AuthenticationProviders":["SAML"],
                   "PermissionType":"SERVICE_MANAGED",
                   "SamlConfiguration":{"IdpMetadata":{"Url":"https://idp.example.com/meta"}}}}},
               "Outputs":{"WorkspaceId":{"Value":{"Ref":"Workspace"}}}}"#,
        ),
        (
            "a declared PluginAdminEnabled",
            "grafana-plugin-stack",
            r#"{"Resources":{"Workspace":{"Type":"AWS::Grafana::Workspace",
                 "Properties":{"AccountAccessType":"CURRENT_ACCOUNT",
                   "AuthenticationProviders":["SAML"],
                   "PermissionType":"SERVICE_MANAGED",
                   "PluginAdminEnabled":true}}},
               "Outputs":{"WorkspaceId":{"Value":{"Ref":"Workspace"}}}}"#,
        ),
    ];

    for (case, stack_name, template) in rows {
        let router = build_router();
        let body = create_and_settle(&router, case, stack_name, template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_FAILED"),
            "case {case:?}: expected the stack to fail rather than create a workspace \
             configured differently from its declaration, got {body}"
        );
        let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Workspace"),
            "case {case:?}: expected the reason to name the resource that failed, got \
             {reason:?}"
        );
        assert!(
            outputs_of(&body).is_empty(),
            "case {case:?}: expected no outputs for a stack that created nothing, got {body}"
        );

        let (status, listed) = grafana(&router, "GET", "/workspaces", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListWorkspaces, got {status} with {listed}"
        );
        let held = listed
            .get("workspaces")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held, 0,
            "case {case:?}: expected Amazon Managed Grafana to hold no workspace, it holds \
             {held}: {listed}"
        );
    }
}
