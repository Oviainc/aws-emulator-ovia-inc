// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreatePermission` and `GetPermission`.
//!
//! The rule the rows below pin: a create mints a customer managed permission in the
//! Region the request was signed for, stores it under an ARN built from the name the
//! request gave it, at its first version — which is its default version — with the
//! policy template the request wrote, and every read reports exactly what was stored.
//! A request that breaks one of the rules the member documentation states is refused
//! with the modelled error that words that refusal, and stores nothing.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH resource type the permission is for, and HOW its template names actions —
//!   one action, a list, a wildcard, a prefixed wildcard, with and without a
//!   `Condition`, in either element case — plus the tags it carries and whether the
//!   request named a `clientToken`;
//! * WHICH rule a refused request breaks: a `resourceType` that is not
//!   `<service-code>:<resource-code>`, one that names no resource type this service
//!   shares, one that is shared only through the permissions Amazon Web Services
//!   manages for it, an action of another service's namespace, a template that cannot
//!   be read as one, a template whose effect is not an allow, a reserved tag key, an
//!   empty `clientToken`, a name already taken in the Region;
//! * WHICH parameter a repeat under one `clientToken` changes, which decides a retry
//!   from a mismatch;
//! * WHAT a read is asked for: the default version, that version by number, a version
//!   that was never written, a version below the first, an unknown permission, and a
//!   value that is not a managed permission ARN.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    FIRST_PERMISSION_VERSION, MAX_POLICY_TEMPLATE_LEN, PERMISSION_FEATURE_SET_STANDARD,
    PERMISSION_STATUS_ATTACHABLE, PERMISSION_TYPE_CUSTOMER_MANAGED, RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// A resource type a customer managed permission can be created for
/// (`RESOURCE_TYPES`), used wherever a row is about something other than the resource
/// type itself.
const CMP_RESOURCE_TYPE: &str = "codebuild:Project";

/// A template that allows two actions of that resource type's service, the shape
/// `CreatePermission$policyTemplate` documents: an `Effect` of `ALLOW` and an
/// `Action` list.
fn project_template() -> Value {
    json!({
        "Effect": "ALLOW",
        "Action": ["codebuild:BatchGetProjects", "codebuild:ListBuildsForProject"],
    })
}

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

/// The ARN a permission of this name must be stored under: the signed-for Region and
/// the canonical emulator account, with the name as the ARN's resource id.
fn expected_permission_arn(name: &str) -> String {
    format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/{name}")
}

/// A member of a reported permission, as a string.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// A boolean member of a reported permission.
fn flag(case: &str, permission: &Value, member: &str) -> bool {
    permission
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
}

/// The `permission` a create or a read reported.
fn permission_of(case: &str, body: &Value) -> Value {
    body.get("permission")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permission, got {body:?}"))
}

/// One row of the round-trip table.
struct CreateCase {
    case: &'static str,
    name: &'static str,
    resource_type: &'static str,
    template: Value,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

#[tokio::test]
async fn a_created_permission_reads_back_through_get_permission() {
    // Every resource type below is a display name `ListResourceTypes` reports that
    // supports customer managed permissions (`RESOURCE_TYPES`), the values
    // `CreatePermission$resourceType` admits — including entries this issue never
    // cited.
    let cases = vec![
        CreateCase {
            case: "a list of two actions of the resource type's service",
            name: "ProjectReader",
            resource_type: CMP_RESOURCE_TYPE,
            template: project_template(),
            tags: vec![],
            client_token: None,
        },
        CreateCase {
            case: "one action named as a bare string",
            name: "GroupReader",
            resource_type: "resource-groups:Group",
            template: json!({"Effect": "ALLOW", "Action": "resource-groups:GetGroup"}),
            tags: vec![("team", "platform")],
            client_token: Some("create-permission-token-1"),
        },
        CreateCase {
            case: "a whole-namespace wildcard action",
            name: "Resolver.Full-1",
            resource_type: "route53resolver:FirewallRuleGroup",
            template: json!({"Effect": "ALLOW", "Action": ["route53resolver:*"]}),
            tags: vec![("env", "test"), ("cost-centre", "42")],
            client_token: None,
        },
        CreateCase {
            case: "a prefixed wildcard action and a condition",
            name: "ImageReader",
            resource_type: "imagebuilder:Image",
            template: json!({
                "Effect": "ALLOW",
                "Action": ["imagebuilder:Get*"],
                "Condition": {"StringEquals": {"aws:PrincipalOrgID": "o-abc1234567"}}
            }),
            tags: vec![],
            client_token: Some("create-permission-token-2"),
        },
        CreateCase {
            case: "template elements cased as the caller wrote them",
            name: "LowercaseElements",
            resource_type: CMP_RESOURCE_TYPE,
            template: json!({"effect": "allow", "action": ["codebuild:BatchGetProjects"]}),
            tags: vec![("only", "")],
            client_token: None,
        },
        CreateCase {
            case: "a resource type whose service code carries a hyphen",
            name: "LicenseReader",
            resource_type: "license-manager:LicenseConfiguration",
            template: json!({
                "Effect": "ALLOW",
                "Action": [
                    "license-manager:ListUsageForLicenseConfiguration",
                    "license-manager:GetLicenseConfiguration",
                ],
            }),
            tags: vec![],
            client_token: None,
        },
        CreateCase {
            case: "a resource type named in another case than its display name",
            name: "CaseInsensitiveType",
            resource_type: "glue:table",
            template: json!({"Effect": "ALLOW", "Action": ["glue:GetTable"]}),
            tags: vec![],
            client_token: None,
        },
    ];

    let router = emulator();
    for CreateCase {
        case,
        name,
        resource_type,
        template,
        tags,
        client_token,
    } in cases
    {
        let template_text = serde_json::to_string(&template).expect("serialize the template");
        let mut request = json!({
            "name": name,
            "resourceType": resource_type,
            "policyTemplate": template_text,
        });
        if !tags.is_empty() {
            request["tags"] = Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            );
        }
        if let Some(token) = client_token {
            request["clientToken"] = json!(token);
        }

        let (status, created) = create_permission(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with {created:?}"
        );
        let reported = permission_of(case, &created);
        let expected_arn = expected_permission_arn(name);
        assert_eq!(
            field(case, &reported, "arn"),
            expected_arn,
            "case {case:?}: create reported the wrong ARN, got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "version"),
            FIRST_PERMISSION_VERSION.to_string(),
            "case {case:?}: a new permission is at its first version, got {reported:?}"
        );
        assert!(
            flag(case, &reported, "defaultVersion"),
            "case {case:?}: the only version of a permission is its default, got {reported:?}"
        );
        assert!(
            !flag(case, &reported, "isResourceTypeDefault"),
            "case {case:?}: a customer managed permission is not its resource type's default, \
             got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "permissionType"),
            PERMISSION_TYPE_CUSTOMER_MANAGED,
            "case {case:?}: create reported the wrong permissionType, got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "featureSet"),
            PERMISSION_FEATURE_SET_STANDARD,
            "case {case:?}: create reported the wrong featureSet, got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "status"),
            PERMISSION_STATUS_ATTACHABLE,
            "case {case:?}: a new permission can be attached to shares, got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "name"),
            name,
            "case {case:?}: create reported the wrong name, got {reported:?}"
        );
        assert_eq!(
            field(case, &reported, "resourceType"),
            resource_type,
            "case {case:?}: create reported the wrong resourceType, got {reported:?}"
        );
        let mut expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        expected_tags.sort();
        assert_eq!(
            tags_of(case, &reported),
            expected_tags,
            "case {case:?}: create reported the wrong tags, got {reported:?}"
        );
        let created_at = timestamp(case, &reported, "creationTime");
        assert_eq!(
            timestamp(case, &reported, "lastUpdatedTime"),
            created_at,
            "case {case:?}: nothing has updated a new permission, got {reported:?}"
        );
        let reported_token = created
            .get("clientToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: create reported no clientToken, got {created:?}")
            });
        match client_token {
            Some(token) => assert_eq!(
                reported_token, token,
                "case {case:?}: create reported another clientToken, got {created:?}"
            ),
            None => assert!(
                !reported_token.is_empty(),
                "case {case:?}: a create that named no clientToken is answered with the \
                 generated one, got {created:?}"
            ),
        }

        // The round trip: the stored record read back through `GetPermission`, which
        // reports the default version when the request names none and the same
        // version when it names it by number.
        for (asked, read_request) in [
            (
                "the default version",
                json!({"permissionArn": expected_arn}),
            ),
            (
                "that version by number",
                json!({
                    "permissionArn": expected_arn,
                    "permissionVersion": FIRST_PERMISSION_VERSION,
                }),
            ),
        ] {
            let (status, read) = get_permission(&router, &read_request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {asked} expected 200, got {status} with {read:?}"
            );
            let detail = permission_of(case, &read);
            assert_eq!(
                field(case, &detail, "permission"),
                template_text,
                "case {case:?}: reading {asked} reported another policy document, got {detail:?}"
            );
            for member in [
                "arn",
                "version",
                "name",
                "resourceType",
                "permissionType",
                "featureSet",
                "status",
            ] {
                assert_eq!(
                    field(case, &detail, member),
                    field(case, &reported, member),
                    "case {case:?}: reading {asked} reported another {member}, got {detail:?}"
                );
            }
            assert!(
                flag(case, &detail, "defaultVersion"),
                "case {case:?}: reading {asked} must report the default version, got {detail:?}"
            );
            assert_eq!(
                tags_of(case, &detail),
                expected_tags,
                "case {case:?}: reading {asked} reported other tags, got {detail:?}"
            );
            assert_eq!(
                timestamp(case, &detail, "creationTime"),
                created_at,
                "case {case:?}: reading {asked} reported another creationTime, got {detail:?}"
            );
        }
    }
}

/// One row of the refusal table: the body, and the error RAM words that refusal with.
struct RefusalCase {
    case: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_create_that_breaks_a_rule_is_refused_and_stores_nothing() {
    let template = serde_json::to_string(&project_template()).expect("serialize the template");
    let cases = vec![
        RefusalCase {
            case: "a resourceType naming no resource code",
            body: json!({"name": "R1", "resourceType": "codebuild", "policyTemplate": template}),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "an empty resourceType",
            body: json!({"name": "R2", "resourceType": "", "policyTemplate": template}),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a resourceType with no service code",
            body: json!({"name": "R3", "resourceType": ":Project", "policyTemplate": template}),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a resourceType with an empty resource code",
            body: json!({"name": "R4", "resourceType": "codebuild:", "policyTemplate": template}),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a resourceType carrying a third segment",
            body: json!({
                "name": "R5",
                "resourceType": "codebuild:Project:extra",
                "policyTemplate": template,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a resourceType separated by a space",
            body: json!({
                "name": "R6",
                "resourceType": "codebuild Project",
                "policyTemplate": template,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a well-formed resourceType of a service this service cannot share",
            body: json!({
                "name": "R23",
                "resourceType": "s3:Bucket",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["s3:GetObject"]}"#,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a resourceType naming no real service",
            body: json!({
                "name": "R25",
                "resourceType": "nosuchservice:NoSuchType",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["nosuchservice:Read"]}"#,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a shareable resource type that takes no customer managed permission",
            body: json!({
                "name": "R26",
                "resourceType": "ec2:Subnet",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["ec2:DescribeSubnets"]}"#,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "another resource type of that same service",
            body: json!({
                "name": "R27",
                "resourceType": "ec2:TransitGateway",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["ec2:DescribeTransitGateways"]}"#,
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "an action of another service's namespace",
            body: json!({
                "name": "R7",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["s3:GetObject"]}"#,
            }),
            status: 400,
            code: "InvalidPolicyException",
        },
        RefusalCase {
            case: "one supported action beside one that is not",
            body: json!({
                "name": "R8",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate":
                    r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects","iam:PassRole"]}"#,
            }),
            status: 400,
            code: "InvalidPolicyException",
        },
        RefusalCase {
            case: "an action that names no service namespace at all",
            body: json!({
                "name": "R9",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":"*"}"#,
            }),
            status: 400,
            code: "InvalidPolicyException",
        },
        RefusalCase {
            case: "a policyTemplate that is not JSON",
            body: json!({
                "name": "R10",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": "allow everything",
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "a policyTemplate that is a JSON array",
            body: json!({
                "name": "R11",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": "[]",
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "a policyTemplate carrying no Effect",
            body: json!({
                "name": "R12",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Action":["codebuild:BatchGetProjects"]}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "a policyTemplate carrying no Action",
            body: json!({
                "name": "R13",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW"}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "an Action that is not an action name",
            body: json!({
                "name": "R14",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":5}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "an empty Action list",
            body: json!({
                "name": "R15",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":[]}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "an Action list with an entry that is not a name",
            body: json!({
                "name": "R16",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:ListProjects",7]}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "a Condition that is not a JSON object",
            body: json!({
                "name": "R17",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate":
                    r#"{"Effect":"ALLOW","Action":["codebuild:ListProjects"],"Condition":"always"}"#,
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "an Effect that denies",
            body: json!({
                "name": "R18",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"DENY","Action":["codebuild:ListProjects"]}"#,
            }),
            status: 400,
            code: "InvalidPolicyException",
        },
        RefusalCase {
            case: "an Effect that is neither allow nor deny",
            body: json!({
                "name": "R19",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"Maybe","Action":["codebuild:ListProjects"]}"#,
            }),
            status: 400,
            code: "InvalidPolicyException",
        },
        RefusalCase {
            case: "a tag key using the prefix AWS reserves",
            body: json!({
                "name": "R20",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "tags": [{"key": "aws:created-by", "value": "me"}],
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a tag with no key",
            body: json!({
                "name": "R21",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "tags": [{"value": "orphan"}],
            }),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "a policyTemplate longer than this service stores",
            body: json!({
                "name": "R28",
                "resourceType": CMP_RESOURCE_TYPE,
                // One action repeated until the document is past the bound.
                "policyTemplate": format!(
                    r#"{{"Effect":"ALLOW","Action":[{}]}}"#,
                    vec![r#""codebuild:ListProjects""#; MAX_POLICY_TEMPLATE_LEN / 10].join(","),
                ),
            }),
            status: 400,
            code: "MalformedPolicyTemplateException",
        },
        RefusalCase {
            case: "an empty clientToken",
            body: json!({
                "name": "R22",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "clientToken": "",
            }),
            status: 400,
            code: "InvalidClientTokenException",
        },
    ];

    let router = emulator();
    for RefusalCase {
        case,
        body,
        status: expected_status,
        code,
    } in cases
    {
        let (status, refused) = create_permission(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert!(
            !error_message(&refused).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {refused:?}"
        );

        // A refused create stores nothing, so the name it asked for is still free.
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .expect("every row names the permission it tried to create");
        let (read_status, read) = get_permission(
            &router,
            &json!({"permissionArn": expected_permission_arn(name)}),
        )
        .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused create must store nothing, got {read_status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "UnknownResourceException",
            "case {case:?}: a refused create must store nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_name_already_taken_in_the_region_is_refused() {
    let router = emulator();
    let first = json!({
        "name": "TakenName",
        "resourceType": CMP_RESOURCE_TYPE,
        "policyTemplate": serde_json::to_string(&project_template()).expect("serialize"),
    });
    let (status, created) = create_permission(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"the first create\": expected 200, got {status} with {created:?}"
    );

    // A different permission under the same name, whatever else it says, is the name
    // clash RAM refuses — including one that only changes the resource type.
    for (case, body) in [
        (
            "the same request again without its clientToken",
            first.clone(),
        ),
        (
            "the same name for another resource type",
            json!({
                "name": "TakenName",
                "resourceType": "resource-groups:Group",
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["resource-groups:GetGroup"]}"#,
            }),
        ),
    ] {
        let (status, refused) = create_permission(&router, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "PermissionAlreadyExistsException",
            "case {case:?}: expected PermissionAlreadyExistsException, got {refused:?}"
        );
        assert!(
            error_message(&refused).contains("TakenName"),
            "case {case:?}: the refusal must name the name that is taken, got {refused:?}"
        );
    }

    // The permission the first call stored is untouched by either refusal.
    let (status, read) = get_permission(
        &router,
        &json!({"permissionArn": expected_permission_arn("TakenName")}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the stored permission\": expected 200, got {status} with {read:?}"
    );
    let detail = permission_of("the stored permission", &read);
    assert_eq!(
        field("the stored permission", &detail, "resourceType"),
        CMP_RESOURCE_TYPE,
        "case \"the stored permission\": a refused create must not change it, got {detail:?}"
    );

    // A free name in the same Region is still a create.
    let (status, created) = create_permission(
        &router,
        &json!({
            "name": "FreeName",
            "resourceType": CMP_RESOURCE_TYPE,
            "policyTemplate": serde_json::to_string(&project_template()).expect("serialize"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a free name\": expected 200, got {status} with {created:?}"
    );
}

#[tokio::test]
async fn one_client_token_creates_one_permission() {
    let template = serde_json::to_string(&project_template()).expect("serialize the template");
    let first = json!({
        "name": "Idempotent",
        "resourceType": CMP_RESOURCE_TYPE,
        "policyTemplate": template,
        "tags": [{"key": "env", "value": "test"}],
        "clientToken": "one-token",
    });

    let router = emulator();
    let (status, created) = create_permission(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"the first create\": expected 200, got {status} with {created:?}"
    );
    let reported = permission_of("the first create", &created);
    let arn = field("the first create", &reported, "arn");
    let created_at = timestamp("the first create", &reported, "creationTime");

    // A retry of the SAME request under the same token acts once: it answers from the
    // stored permission, with the instant the first call stored.
    let (status, repeated) = create_permission(&router, &first).await;
    assert_eq!(
        status, 200,
        "case \"a retry\": expected 200, got {status} with {repeated:?}"
    );
    let repeated_permission = permission_of("a retry", &repeated);
    assert_eq!(
        field("a retry", &repeated_permission, "arn"),
        arn,
        "case \"a retry\": a retry answers from the stored permission, got {repeated_permission:?}"
    );
    assert_eq!(
        timestamp("a retry", &repeated_permission, "creationTime"),
        created_at,
        "case \"a retry\": a retry creates nothing further, got {repeated_permission:?}"
    );

    // The same token with any other parameter changed is the mismatch, whichever
    // parameter changed.
    let mismatches = vec![
        (
            "another policyTemplate",
            json!({
                "name": "Idempotent",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:ListProjects"]}"#,
                "tags": [{"key": "env", "value": "test"}],
                "clientToken": "one-token",
            }),
        ),
        (
            "another resourceType",
            json!({
                "name": "Idempotent",
                "resourceType": "codebuild:ReportGroup",
                "policyTemplate": template,
                "tags": [{"key": "env", "value": "test"}],
                "clientToken": "one-token",
            }),
        ),
        (
            "another name",
            json!({
                "name": "Idempotent2",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "tags": [{"key": "env", "value": "test"}],
                "clientToken": "one-token",
            }),
        ),
        (
            "another tag value",
            json!({
                "name": "Idempotent",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "tags": [{"key": "env", "value": "prod"}],
                "clientToken": "one-token",
            }),
        ),
        (
            "no tags at all",
            json!({
                "name": "Idempotent",
                "resourceType": CMP_RESOURCE_TYPE,
                "policyTemplate": template,
                "clientToken": "one-token",
            }),
        ),
    ];
    for (case, body) in mismatches {
        let (status, refused) = create_permission(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "IdempotentParameterMismatchException",
            "case {case:?}: expected IdempotentParameterMismatchException, got {refused:?}"
        );
    }

    // Another token and another name is another permission.
    let (status, second) = create_permission(
        &router,
        &json!({
            "name": "Idempotent2",
            "resourceType": CMP_RESOURCE_TYPE,
            "policyTemplate": template,
            "clientToken": "another-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another token\": expected 200, got {status} with {second:?}"
    );
    assert_ne!(
        field(
            "another token",
            &permission_of("another token", &second),
            "arn"
        ),
        arn,
        "case \"another token\": a second permission is stored under its own ARN, got {second:?}"
    );
}

#[tokio::test]
async fn the_resource_type_catalog_decides_which_types_take_a_permission() {
    let router = emulator();
    // The invariant over the WHOLE catalog, not only the rows above: a resource type
    // this service shares takes a customer managed permission exactly when the
    // catalog records that it supports one, and the created permission reports the
    // type it was created for.
    for (index, entry) in RESOURCE_TYPES.iter().enumerate() {
        let case = entry.resource_type;
        let service = case
            .split(':')
            .next()
            .expect("every catalog entry names a service code");
        let name = format!("CatalogEntry{index}");
        let template = format!(r#"{{"Effect":"ALLOW","Action":["{service}:*"]}}"#);
        let (status, answered) = create_permission(
            &router,
            &json!({
                "name": name,
                "resourceType": case,
                "policyTemplate": template,
            }),
        )
        .await;

        if !entry.customer_managed_permissions {
            assert_eq!(
                status, 400,
                "case {case:?}: a resource type with no customer managed permission support must \
                 be refused, got {status} with {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {answered:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answered:?}"
        );
        assert_eq!(
            field(case, &permission_of(case, &answered), "resourceType"),
            case,
            "case {case:?}: create reported another resourceType, got {answered:?}"
        );

        let (status, read) = get_permission(
            &router,
            &json!({"permissionArn": expected_permission_arn(&name)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading it back expected 200, got {status} with {read:?}"
        );
        assert_eq!(
            field(case, &permission_of(case, &read), "permission"),
            template,
            "case {case:?}: the read reported another policy document, got {read:?}"
        );
    }
}

#[tokio::test]
async fn the_unsupported_resource_type_refusal_names_the_type_it_was_asked_for() {
    let router = emulator();
    // The message is the template RAM words this refusal with, its variable part
    // taken from the request: the resource type the create asked for, whichever one
    // it is and however it was cased.
    for (index, resource_type) in ["ec2:Subnet", "ec2:TransitGateway", "ec2:prefixlist"]
        .into_iter()
        .enumerate()
    {
        let service = resource_type
            .split(':')
            .next()
            .expect("each row names a service code");
        let (status, refused) = create_permission(
            &router,
            &json!({
                "name": format!("Unsupported{index}"),
                "resourceType": resource_type,
                "policyTemplate": format!(r#"{{"Effect":"ALLOW","Action":["{service}:Describe*"]}}"#),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidParameterException",
            "case {resource_type:?}: expected InvalidParameterException, got {refused:?}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Customer managed permission is not supported for resource type {resource_type}"),
            "case {resource_type:?}: the refusal is worded as RAM words it, got {refused:?}"
        );
    }
}

/// One row of the read-refusal table.
struct ReadRefusalCase {
    case: &'static str,
    body: Value,
    code: &'static str,
}

#[tokio::test]
async fn a_read_of_a_permission_nothing_stored_is_refused() {
    let router = emulator();
    let (status, created) = create_permission(
        &router,
        &json!({
            "name": "Readable",
            "resourceType": CMP_RESOURCE_TYPE,
            "policyTemplate": serde_json::to_string(&project_template()).expect("serialize"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the create under test\": expected 200, got {status} with {created:?}"
    );
    let stored = expected_permission_arn("Readable");

    let cases = vec![
        ReadRefusalCase {
            case: "a value that is not an ARN",
            body: json!({"permissionArn": "Readable"}),
            code: "MalformedArnException",
        },
        ReadRefusalCase {
            case: "an ARN of this service's resource-share resource",
            body: json!({
                "permissionArn":
                    format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/11111111-2222-3333-4444-555555555555"),
            }),
            code: "MalformedArnException",
        },
        ReadRefusalCase {
            case: "a permission ARN of another service",
            body: json!({"permissionArn": format!("arn:aws:iam::{ACCOUNT}:permission/Readable")}),
            code: "MalformedArnException",
        },
        ReadRefusalCase {
            case: "a permission ARN with no name after its prefix",
            body: json!({"permissionArn": format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/")}),
            code: "MalformedArnException",
        },
        ReadRefusalCase {
            case: "a well-formed ARN no permission is stored under",
            body: json!({"permissionArn": expected_permission_arn("NeverCreated")}),
            code: "UnknownResourceException",
        },
        ReadRefusalCase {
            case: "a version that was never written",
            body: json!({"permissionArn": stored, "permissionVersion": 2}),
            code: "UnknownResourceException",
        },
        ReadRefusalCase {
            case: "a version below the first one",
            body: json!({"permissionArn": stored, "permissionVersion": 0}),
            code: "InvalidParameterException",
        },
        ReadRefusalCase {
            case: "a negative version",
            body: json!({"permissionArn": stored, "permissionVersion": -1}),
            code: "InvalidParameterException",
        },
    ];

    for ReadRefusalCase { case, body, code } in cases {
        let (status, refused) = get_permission(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        assert!(
            !error_message(&refused).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {refused:?}"
        );
    }
}
