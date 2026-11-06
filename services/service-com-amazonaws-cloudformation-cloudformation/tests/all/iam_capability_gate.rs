// SPDX-License-Identifier: CC0-1.0

//! The capability acknowledgement an operation that SUBMITS a template is held to.
//!
//! RULE: an operation that submits a template refuses the request, before anything is
//! written or deployed, when the request's own `Capabilities` do not acknowledge what the
//! template's declarations require — the acknowledgement of a template's IAM resources,
//! and its NAMED form when the template pins one of their names — and the refusal is the
//! modelled `InsufficientCapabilitiesException` with HTTP 400
//! (`CreateStackInput$Capabilities`: "you must explicitly acknowledge this by specifying
//! one of these capabilities ... If you have IAM resources with custom names, you *must*
//! specify `CAPABILITY_NAMED_IAM`. If you don't specify either of these capabilities,
//! CloudFormation returns an `InsufficientCapabilities` error").
//!
//! The rows differ along what the rule generalises over: which acknowledgement the request
//! carries (none, the unnamed form, the named form, an unrelated one), which of the
//! catalogued IAM types the template declares and whether that type HAS a custom-name
//! property, whether the template pins a name, whether the resource stands under a
//! `Condition`, how the template is spelled (JSON and YAML), a template with no IAM
//! resource at all, and an IAM resource standing beside a resource of another service. The
//! issue's own case — one `AWS::IAM::Role` with an explicit `RoleName`, submitted with the
//! unnamed acknowledgement only — is ONE row.
//!
//! The catalogue the rows draw their types and name properties from is "Acknowledging IAM
//! resources in CloudFormation templates" in the CloudFormation User Guide, which lists the
//! resource types requiring the acknowledgement and says a custom name for one of them
//! requires the named form.
//!
//! The refusal is TOTAL: the second test creates nothing — no stack and no role in IAM —
//! and then deploys the same template once the request acknowledges it, which is the
//! positive control. The last two tests carry the rule onto the other operations that
//! submit a template: `UpdateStack` and `CreateChangeSet`.
//!
//! Everything goes over the awsQuery wire of the router `stack_lifecycle.rs` builds, which
//! mounts IAM beside CloudFormation on one engine — so the role an accepted stack creates
//! is read back through IAM's own API.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, iam, member, router,
    wait_for_the_transition, xml_text, xml_texts,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The wire code a request that withheld a required capability is refused under.
const INSUFFICIENT: &str = "InsufficientCapabilitiesException";

/// The acknowledgement of a template's IAM resources when CloudFormation names them.
const IAM: &str = "CAPABILITY_IAM";

/// The acknowledgement of a template's IAM resources when the template names one of them.
const NAMED_IAM: &str = "CAPABILITY_NAMED_IAM";

/// A third `Capability` value, which acknowledges a template's macros rather than its IAM
/// resources (the model's `Capability` enum carries exactly these three values).
const AUTO_EXPAND: &str = "CAPABILITY_AUTO_EXPAND";

/// A trust policy an `AWS::IAM::Role` declares inline, as a template object: IAM's own
/// `CreateRole` requires the document, so a row whose stack must complete declares one.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// A JSON template whose `Resources` section is `resources`.
fn template(resources: Value) -> String {
    json!({"AWSTemplateFormatVersion": "2010-09-09", "Resources": resources}).to_string()
}

/// One row of the table: the template submitted, the acknowledgement the request carries,
/// and what the service must answer.
struct Case {
    case: &'static str,
    template: String,
    /// The request's `Capabilities`, in the order it sends them.
    capabilities: &'static [&'static str],
    /// The capability the refusal must name, or `None` when the request is accepted.
    refused_for: Option<&'static str>,
}

/// The table. Each row's template declares what the row's name says and nothing else, so
/// the acknowledgement it requires is computed from the declaration rather than from the
/// stack name.
fn cases() -> Vec<Case> {
    vec![
        // The issue's own case: a role the template NAMES, acknowledged as if it were not.
        Case {
            case: "a template-named role, acknowledged only as a generated-name one",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "gate-named-role",
                    "AssumeRolePolicyDocument": trust_policy(),
                },
            }})),
            capabilities: &[IAM],
            refused_for: Some(NAMED_IAM),
        },
        Case {
            case: "a template-named role, acknowledged as a named one",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "gate-accepted-role",
                    "AssumeRolePolicyDocument": trust_policy(),
                },
            }})),
            capabilities: &[NAMED_IAM],
            refused_for: None,
        },
        Case {
            case: "a generated-name role with nothing acknowledged",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {"AssumeRolePolicyDocument": trust_policy()},
            }})),
            capabilities: &[],
            refused_for: Some(IAM),
        },
        Case {
            case: "a generated-name role acknowledged as a generated-name one",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {"AssumeRolePolicyDocument": trust_policy()},
            }})),
            capabilities: &[IAM],
            refused_for: None,
        },
        // "If you have IAM resources, you can specify either capability": the named form is
        // the stronger acknowledgement, so it covers a template that names none of them.
        Case {
            case: "a generated-name role acknowledged as a named one",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {"AssumeRolePolicyDocument": trust_policy()},
            }})),
            capabilities: &[NAMED_IAM],
            refused_for: None,
        },
        // Another catalogue entry with a custom-name property (`AWS::IAM::User` /
        // `UserName`), beyond the role the issue cited.
        Case {
            case: "a template-named user, acknowledged only as a generated-name one",
            template: template(json!({"User": {
                "Type": "AWS::IAM::User",
                "Properties": {"UserName": "gate-named-user"},
            }})),
            capabilities: &[IAM],
            refused_for: Some(NAMED_IAM),
        },
        // And a third (`AWS::IAM::ManagedPolicy` / `ManagedPolicyName`).
        Case {
            case: "a template-named managed policy, acknowledged only as a generated-name one",
            template: template(json!({"Policy": {
                "Type": "AWS::IAM::ManagedPolicy",
                "Properties": {"ManagedPolicyName": "gate-named-policy"},
            }})),
            capabilities: &[IAM],
            refused_for: Some(NAMED_IAM),
        },
        // A catalogued type with NO custom-name property: `PolicyName` is the name of the
        // inline policy, not of an IAM identity, so the unnamed acknowledgement is enough.
        // The declaration carries what the type requires of one — the policy's name and its
        // document — because an `AWS::IAM::Policy` the stack deploys is really embedded in
        // the identities it names, in IAM; the user it names is declared here (with a
        // generated name, so the case still acknowledges only the unnamed capability).
        Case {
            case: "an inline policy, a type with no custom-name property, acknowledged as unnamed",
            template: template(json!({
                "Someone": {"Type": "AWS::IAM::User"},
                "Inline": {
                    "Type": "AWS::IAM::Policy",
                    "Properties": {
                        "PolicyName": "gate-inline-policy",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "sqs:ReceiveMessage",
                                "Resource": "*"
                            }]
                        },
                        "Users": [{"Ref": "Someone"}],
                    },
                },
            })),
            capabilities: &[IAM],
            refused_for: None,
        },
        Case {
            case: "an access key, a type with no custom-name property, nothing acknowledged",
            template: template(json!({"Key": {
                "Type": "AWS::IAM::AccessKey",
                "Properties": {"UserName": "someone"},
            }})),
            capabilities: &[],
            refused_for: Some(IAM),
        },
        // An acknowledgement of something else is no acknowledgement of the IAM resources.
        Case {
            case: "a generated-name role acknowledged only for macro expansion",
            template: template(json!({"Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {"AssumeRolePolicyDocument": trust_policy()},
            }})),
            capabilities: &[AUTO_EXPAND],
            refused_for: Some(IAM),
        },
        // The acknowledgement is about what the template CONTAINS, which is settled before
        // any condition is evaluated.
        Case {
            case: "a template-named role standing under a Condition",
            template: json!({
                "Conditions": {"Never": {"Fn::Equals": ["a", "b"]}},
                "Resources": {"Role": {
                    "Type": "AWS::IAM::Role",
                    "Condition": "Never",
                    "Properties": {
                        "RoleName": "gate-conditional-role",
                        "AssumeRolePolicyDocument": trust_policy(),
                    },
                }},
            })
            .to_string(),
            capabilities: &[IAM],
            refused_for: Some(NAMED_IAM),
        },
        // The same declaration in the other spelling a template may arrive in.
        Case {
            case: "a template-named role spelled in YAML",
            template: "Resources:\n\
                 \x20 Role:\n\
                 \x20   Type: AWS::IAM::Role\n\
                 \x20   Properties:\n\
                 \x20     RoleName: gate-yaml-role\n"
                .to_string(),
            capabilities: &[IAM],
            refused_for: Some(NAMED_IAM),
        },
        // A template that requires no acknowledgement at all is accepted with none.
        Case {
            case: "a template with no IAM resource and nothing acknowledged",
            template: template(json!({
                "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
            })),
            capabilities: &[],
            refused_for: None,
        },
        Case {
            case: "a bucket beside a generated-name role, acknowledged as unnamed",
            template: template(json!({
                "Bucket": {"Type": "AWS::S3::Bucket"},
                "Role": {
                    "Type": "AWS::IAM::Role",
                    "Properties": {"AssumeRolePolicyDocument": trust_policy()},
                },
            })),
            capabilities: &[IAM],
            refused_for: None,
        },
    ]
}

/// The `Capabilities.member.N` members a request carries for `capabilities`, as an SDK
/// flattens the list.
fn capability_members(capabilities: &[&str]) -> Vec<(String, String)> {
    capabilities
        .iter()
        .enumerate()
        .map(|(index, capability)| {
            member(&format!("Capabilities.member.{}", index + 1), capability)
        })
        .collect()
}

/// Assert a response is the capability refusal: HTTP 400, the modelled wire code, and a
/// message naming the capability the request withheld.
fn assert_refused(case: &str, status: u16, body: &str, missing: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: an unacknowledged template must be refused, got {status}: {body}"
    );
    let code = xml_text(body, "Code");
    assert_eq!(
        code.as_deref(),
        Some(INSUFFICIENT),
        "case {case:?}: expected the wire code {INSUFFICIENT}, got {code:?}: {body}"
    );
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        message.contains(missing),
        "case {case:?}: the refusal must name the capability the request withheld \
         ({missing}), got {message:?}"
    );
}

/// Assert the service holds no stack under `name`: the `ValidationError` a read of an
/// absent stack answers.
async fn assert_no_such_stack(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = describe_stack(router, name).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a refused create must leave no stack to describe, got {status}: {body}"
    );
    let code = xml_text(&body, "Code");
    assert_eq!(
        code.as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected the absent-stack code ValidationError, got {code:?}: {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert!(
        message.contains(name),
        "case {case:?}: the absent-stack message must name the stack asked for, got {message:?}"
    );
}

/// The gate itself, over the table: a `CreateStack` whose `Capabilities` fall short of what
/// the template requires is refused and creates nothing, and one that acknowledges the
/// template deploys it.
#[tokio::test]
async fn create_stack_is_held_to_the_capabilities_the_request_acknowledges() {
    let router = router();

    for (index, case) in cases().into_iter().enumerate() {
        let stack_name = format!("gate-{index}");
        let (status, body) = create_stack(
            &router,
            &stack_name,
            &case.template,
            &capability_members(case.capabilities),
        )
        .await;

        match case.refused_for {
            Some(missing) => {
                assert_refused(case.case, status, &body, missing);
                assert!(
                    xml_text(&body, "StackId").is_none(),
                    "case {:?}: a refused create must report no StackId, got {body}",
                    case.case
                );
                assert_no_such_stack(&router, case.case, &stack_name).await;
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: an acknowledged template must be accepted, got {status}: {body}",
                    case.case
                );
                assert!(
                    xml_text(&body, "StackId").is_some(),
                    "case {:?}: an accepted create must report a StackId, got {body}",
                    case.case
                );
                wait_for_the_transition().await;
                let (status, described) = describe_stack(&router, &stack_name).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the created stack must be readable, got {status}: {described}",
                    case.case
                );
                assert_eq!(
                    xml_text(&described, "StackStatus").as_deref(),
                    Some(status::CREATE_COMPLETE),
                    "case {:?}: the acknowledged stack must complete, got {described}",
                    case.case
                );
            }
        }
    }
}

/// The refusal is TOTAL: nothing the template declares is created, and the same template
/// deploys once the request acknowledges it (the positive control).
#[tokio::test]
async fn a_refused_create_creates_neither_the_stack_nor_the_role_the_template_declares() {
    let router = router();
    let role_name = "gate-total-refusal-role";
    let body_template = template(json!({"Role": {
        "Type": "AWS::IAM::Role",
        "Properties": {
            "RoleName": role_name,
            "AssumeRolePolicyDocument": trust_policy(),
        },
    }}));

    let (status, refused) = create_stack(
        &router,
        "gate-total",
        &body_template,
        &capability_members(&[IAM]),
    )
    .await;
    assert_refused("the named role with the unnamed acknowledgement", status, &refused, NAMED_IAM);
    assert_no_such_stack(&router, "the refused create", "gate-total").await;

    let (status, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 404,
        "a refused create must create no role in IAM, got {status}: {body}"
    );
    let codes = xml_texts(&body, "Code");
    assert!(
        codes.iter().any(|code| code == "NoSuchEntity"),
        "expected IAM's own absent-entity code for the role, got {codes:?} in {body}"
    );

    // The positive control: the SAME template, acknowledged.
    let (status, created) = create_stack(
        &router,
        "gate-total",
        &body_template,
        &capability_members(&[NAMED_IAM]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the acknowledged create must be accepted, got {status}: {created}"
    );
    wait_for_the_transition().await;
    let (status, described) = describe_stack(&router, "gate-total").await;
    assert_eq!(status, 200, "DescribeStacks answered {status}: {described}");
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the acknowledged stack must complete, got {described}"
    );
    let (status, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "the acknowledged stack must have created the role in IAM, got {status}: {body}"
    );
}

/// `UpdateStack` is held to the same acknowledgement, and a refused update leaves the
/// stack's template as it was.
#[tokio::test]
async fn update_stack_is_held_to_the_capabilities_the_request_acknowledges() {
    let router = router();
    let before = template(json!({
        "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
    }));
    let after = template(json!({
        "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
        "Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {"AssumeRolePolicyDocument": trust_policy()},
        },
    }));

    let (status, created) = create_stack(&router, "gate-update", &before, &[]).await;
    assert_eq!(
        status, 200,
        "the template requiring no acknowledgement must be accepted, got {status}: {created}"
    );
    wait_for_the_transition().await;

    let (status, refused) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "gate-update"),
            member("TemplateBody", &after),
        ],
    )
    .await;
    assert_refused("an update adding an unacknowledged role", status, &refused, IAM);

    let (status, kept) = get_template(&router, "gate-update", None).await;
    assert_eq!(status, 200, "GetTemplate answered {status}: {kept}");
    let stored = xml_text(&kept, "TemplateBody").unwrap_or_default();
    assert!(
        !stored.contains("AWS::IAM::Role"),
        "a refused update must leave the stack's template alone, got {stored:?}"
    );

    let mut members = vec![
        member("StackName", "gate-update"),
        member("TemplateBody", &after),
    ];
    members.extend(capability_members(&[IAM]));
    let (status, updated) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status, 200,
        "the acknowledged update must be accepted, got {status}: {updated}"
    );
    wait_for_the_transition().await;
    let (status, described) = describe_stack(&router, "gate-update").await;
    assert_eq!(status, 200, "DescribeStacks answered {status}: {described}");
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "the acknowledged update must complete, got {described}"
    );
}

/// `CreateChangeSet` is held to it too: a plan over a template nobody acknowledged is not
/// written, and the stack the CREATE change set would have minted is not there.
#[tokio::test]
async fn create_change_set_is_held_to_the_capabilities_the_request_acknowledges() {
    let router = router();
    let body_template = template(json!({"Role": {
        "Type": "AWS::IAM::Role",
        "Properties": {
            "RoleName": "gate-change-set-role",
            "AssumeRolePolicyDocument": trust_policy(),
        },
    }}));
    let plan = |capabilities: &'static [&str]| {
        let mut members = vec![
            member("StackName", "gate-change-set"),
            member("ChangeSetName", "gate-plan"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &body_template),
        ];
        members.extend(capability_members(capabilities));
        members
    };

    let (status, refused) = call(&router, "CreateChangeSet", &plan(&[IAM])).await;
    assert_refused("a change set over an unacknowledged template", status, &refused, NAMED_IAM);
    assert_no_such_stack(&router, "the refused change set", "gate-change-set").await;

    let (status, created) = call(&router, "CreateChangeSet", &plan(&[NAMED_IAM])).await;
    assert_eq!(
        status, 200,
        "the acknowledged change set must be accepted, got {status}: {created}"
    );
    assert!(
        xml_text(&created, "Id").is_some(),
        "the accepted change set must report its Id, got {created}"
    );
}
