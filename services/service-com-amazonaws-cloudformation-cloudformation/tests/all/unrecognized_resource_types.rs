// SPDX-License-Identifier: CC0-1.0

//! Resource-type validation: what types a template may declare, over the awsQuery wire.
//!
//! The rule under test: **an operation that submits a template refuses a body whose
//! `Resources` section declares a resource type CloudFormation does not recognise** — HTTP
//! 400 with the wire error code `ValidationError` and the recorded message `Template format
//! error: Unrecognized resource types: [<types>]` — and creates nothing, while a body whose every type is one CloudFormation publishes (or a
//! custom resource) is admitted and deploys as before. The recognised set is the catalogue
//! in `state::resource_types`, taken from the "AWS resource and property types reference"
//! of the CloudFormation User Guide; a service that publishes no resource type at all has
//! no entry there, which is why `AWS::SWF::Domain` — an Amazon Simple Workflow Service
//! domain, which only that service's own `RegisterDomain` creates — is refused.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client sees. The table spans the class the rule generalises over rather than the
//! reported case alone: a namespace with no resource types, a misspelled resource in a real
//! namespace, a namespace that is no service, a catalogued type in the wrong case, several
//! unrecognised types in one template, an unrecognised type beside a catalogued one, a
//! third-party extension nobody activated, both template spellings (JSON and YAML), a
//! resource declared under a condition, and — on the accepted side — catalogued types this
//! crate's other suites do not cite, a custom resource under both of its spellings, a
//! template whose `Transform` may expand into types of its own, and a template declaring no
//! resources at all.
//!
//! The same rule is then driven through the other operations that submit a template for
//! deployment — `CreateChangeSet` and `UpdateStack` — and `ValidateTemplate`, whose
//! documented scope is the body's FORM, is held to the opposite: it answers a verdict for a
//! well-formed template whatever types it declares.

use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::
    IMPLEMENTED_RESOURCE_TYPES;

use crate::stack_lifecycle::{
    assert_stack_id_shape, call, create_stack, describe_stack, get_template, member, router,
    router_with_engine, store, wait_for_the_transition, xml_text,
};

/// The fixed form of the refusal, as CloudFormation words it.
const MESSAGE_PREFIX: &str = "Template format error: Unrecognized resource types: [";

/// The resource types the refusal names, sorted.
///
/// Sorted because the ORDER the reader walked the `Resources` section in is not part of the
/// rule: a parsed JSON object iterates in insertion order or in sorted order depending on
/// whether anything in the build enabled `serde_json`'s `preserve_order` feature, so the
/// set is what a client can depend on.
fn named_types(message: &str) -> Vec<String> {
    let inner = message
        .strip_prefix(MESSAGE_PREFIX)
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_default();
    let mut types: Vec<String> = inner
        .split(", ")
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect();
    types.sort();
    types
}

/// The same list from the expectation side.
fn sorted(types: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = types.iter().map(|entry| (*entry).to_string()).collect();
    sorted.sort();
    sorted
}

/// Assert one response is the refusal this rule owes, naming exactly `expected`.
fn assert_refused(case: &str, status: u16, body: &str, expected: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for an unrecognized resource type, got {status} \
         with body {body}"
    );
    let code = xml_text(body, "Code");
    assert_eq!(
        code.as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected the wire code ValidationError, got {code:?} in {body}"
    );
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        message.starts_with(MESSAGE_PREFIX),
        "case {case:?}: expected a message starting {MESSAGE_PREFIX:?}, got {message:?}"
    );
    assert_eq!(
        named_types(&message),
        sorted(expected),
        "case {case:?}: expected the message to name {:?}, got {message:?}",
        sorted(expected)
    );
}

/// One row of the table: a template body, and the types a refusal must name — or none, for
/// a body that must be admitted.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The unrecognised types, as a set. Empty: the template must be accepted.
    unrecognized: &'static [&'static str],
}

/// The template the issue's own check submits: a declared parameter, one resource of the
/// non-existent type, and an output referencing it.
fn swf_domain_template() -> String {
    r#"{"Parameters": {"DomainName": {"Type": "String"}},
        "Resources": {"SwfDomain": {"Type": "AWS::SWF::Domain",
                                    "Properties": {"Name": {"Ref": "DomainName"},
                                                   "WorkflowExecutionRetentionPeriodInDays": "1"}}},
        "Outputs": {"DomainName": {"Value": {"Ref": "SwfDomain"}}}}"#
        .to_string()
}

/// The class the rule covers. The reported case is the first row.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a namespace CloudFormation publishes no resource type for",
            stack_name: "swf-domain-stack",
            template: swf_domain_template(),
            unrecognized: &["AWS::SWF::Domain"],
        },
        Case {
            case: "a resource name that does not exist in a namespace that does",
            stack_name: "misspelled-resource",
            template: r#"{"Resources":{"B":{"Type":"AWS::S3::Buckets","Properties":{"BucketName":"b"}}}}"#
                .to_string(),
            unrecognized: &["AWS::S3::Buckets"],
        },
        Case {
            case: "a namespace that is no service at all",
            stack_name: "not-a-service",
            template: r#"{"Resources":{"X":{"Type":"AWS::NotAService::NotAType"}}}"#.to_string(),
            unrecognized: &["AWS::NotAService::NotAType"],
        },
        Case {
            case: "a catalogued type spelled in the wrong case",
            stack_name: "wrong-case",
            template: r#"{"Resources":{"B":{"Type":"aws::s3::bucket"}}}"#.to_string(),
            unrecognized: &["aws::s3::bucket"],
        },
        Case {
            case: "several unrecognised types in one template, each named once",
            stack_name: "several-unknowns",
            template: r#"{"Resources":{"A":{"Type":"AWS::SWF::Domain"},
                           "B":{"Type":"AWS::SWF::WorkflowType"},
                           "C":{"Type":"AWS::SWF::Domain"}}}"#
                .to_string(),
            unrecognized: &["AWS::SWF::Domain", "AWS::SWF::WorkflowType"],
        },
        Case {
            case: "an unrecognised type beside a catalogued one",
            stack_name: "mixed-template",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"t"}},
                           "Domain":{"Type":"AWS::SWF::Domain"}}}"#
                .to_string(),
            unrecognized: &["AWS::SWF::Domain"],
        },
        Case {
            case: "a third-party extension this account never activated",
            stack_name: "third-party-extension",
            template: r#"{"Resources":{"P":{"Type":"MyOrg::Analytics::Pipeline"}}}"#.to_string(),
            unrecognized: &["MyOrg::Analytics::Pipeline"],
        },
        Case {
            case: "a module nobody registered",
            stack_name: "unregistered-module",
            template: r#"{"Resources":{"M":{"Type":"MyOrg::Analytics::Pipeline::MODULE"}}}"#
                .to_string(),
            unrecognized: &["MyOrg::Analytics::Pipeline::MODULE"],
        },
        Case {
            case: "the YAML spelling of a template declaring an unrecognised type",
            stack_name: "yaml-unknown",
            template: concat!(
                "Resources:\n",
                "  Fn:\n",
                "    Type: AWS::Lambda::Functions\n",
                "    Properties:\n",
                "      FunctionName: a-function\n",
            )
            .to_string(),
            unrecognized: &["AWS::Lambda::Functions"],
        },
        Case {
            case: "an unrecognised type the template declares under a condition",
            stack_name: "conditional-unknown",
            template: r#"{"Parameters":{"Stage":{"Type":"String","Default":"dev"}},
                          "Conditions":{"IsProd":{"Fn::Equals":[{"Ref":"Stage"},"prod"]}},
                          "Resources":{"D":{"Type":"AWS::SWF::Domain","Condition":"IsProd"}}}"#
                .to_string(),
            unrecognized: &["AWS::SWF::Domain"],
        },
        Case {
            case: "catalogued types across namespaces, none of them provisioned here",
            stack_name: "catalogued-types",
            template: r#"{"Resources":{"Key":{"Type":"AWS::KMS::Key"},
                           "Machine":{"Type":"AWS::StepFunctions::StateMachine","Properties":{"StateMachineName":"m"}},
                           "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.0.0.0/16"}},
                           "Catalog":{"Type":"AWS::Glue::Database"},
                           "Rule":{"Type":"AWS::Events::Rule","Properties":{"Name":"r"}},
                           "Vault":{"Type":"AWS::Backup::BackupVault","Properties":{"BackupVaultName":"v"}},
                           "Pool":{"Type":"AWS::Cognito::UserPool"},
                           "Zone":{"Type":"AWS::Route53::HostedZone"}}}"#
                .to_string(),
            unrecognized: &[],
        },
        Case {
            case: "a custom resource, whose name is the author's own",
            stack_name: "custom-resource",
            template: r#"{"Resources":{"D":{"Type":"Custom::DomainProvisioner",
                           "Properties":{"ServiceToken":"arn:aws:sns:us-east-1:000000000000:provisioner"}}}}"#
                .to_string(),
            unrecognized: &[],
        },
        Case {
            case: "the generic spelling of a custom resource",
            stack_name: "generic-custom-resource",
            template: r#"{"Resources":{"D":{"Type":"AWS::CloudFormation::CustomResource",
                           "Properties":{"ServiceToken":"arn:aws:sns:us-east-1:000000000000:provisioner"}}}}"#
                .to_string(),
            unrecognized: &[],
        },
        Case {
            case: "a template whose Transform may expand into types of its own",
            stack_name: "transformed-template",
            template: r#"{"Transform":"AWS::Serverless-2016-10-31",
                          "Resources":{"Expanded":{"Type":"MyOrg::Macro::Output"}}}"#
                .to_string(),
            unrecognized: &[],
        },
        Case {
            case: "the YAML spelling of a template declaring catalogued types",
            stack_name: "yaml-catalogued",
            template: concat!(
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: a-topic\n",
                "  Alarm:\n",
                "    Type: AWS::CloudWatch::Alarm\n",
                "    Properties:\n",
                "      AlarmName: an-alarm\n",
            )
            .to_string(),
            unrecognized: &[],
        },
        Case {
            case: "a template declaring no resources at all",
            stack_name: "no-resources",
            template: "{}".to_string(),
            unrecognized: &[],
        },
    ]
}

/// `CreateStack` refuses every template declaring a type CloudFormation does not
/// recognise, creating nothing — and admits every template whose types it does.
#[tokio::test]
async fn create_stack_is_held_to_the_published_resource_type_catalogue() {
    let (router, engine) = router_with_engine();
    for case in cases() {
        let Case {
            case,
            stack_name,
            template,
            unrecognized,
        } = case;
        let (status, body) = create_stack(&router, stack_name, &template, &[]).await;
        if unrecognized.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 for a template whose types are all \
                 recognized, got {status} with body {body}"
            );
            let stack_id = xml_text(&body, "StackId").unwrap_or_default();
            assert_stack_id_shape(case, &stack_id, stack_name);
            // Read the stack back through the implemented readers: a created stack is one
            // a later caller sees, with the body it was created from.
            let (described, described_body) = describe_stack(&router, stack_name).await;
            assert_eq!(
                described, 200,
                "case {case:?}: expected DescribeStacks to report the created stack, got \
                 {described} with body {described_body}"
            );
            assert_eq!(
                xml_text(&described_body, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {case:?}: expected DescribeStacks to report {stack_id:?}, got body \
                 {described_body}"
            );
            let (templated, template_body) = get_template(&router, stack_name).await;
            assert_eq!(
                templated, 200,
                "case {case:?}: expected GetTemplate to answer for the created stack, got \
                 {templated} with body {template_body}"
            );
            assert_eq!(
                xml_text(&template_body, "TemplateBody").as_deref(),
                Some(template.as_str()),
                "case {case:?}: expected the stored template to be the submitted body, got \
                 {template_body}"
            );
            continue;
        }
        assert_refused(case, status, &body, unrecognized);
        // Nothing was created: neither the reads nor the store know the name.
        let (described, described_body) = describe_stack(&router, stack_name).await;
        assert_eq!(
            described, 400,
            "case {case:?}: expected the refused stack not to exist, DescribeStacks \
             answered {described} with body {described_body}"
        );
        let held = store(&engine)
            .lock()
            .stacks
            .iter()
            .filter(|stack| stack.stack_name == stack_name)
            .count();
        assert_eq!(
            held, 0,
            "case {case:?}: expected no stored stack named {stack_name:?}, found {held}"
        );
    }
}

/// `ValidateTemplate` is NOT held to the catalogue: its documented scope is whether the
/// body is valid JSON or YAML, and a template declaring an unrecognised type is still
/// well-formed — the type is what refuses the stack operation that submits it, not this
/// verdict on the body.
#[tokio::test]
async fn validate_template_reports_on_the_form_of_the_body_not_its_resource_types() {
    let router = router();
    for case in cases() {
        // A body that declares no resource at all is refused by ValidateTemplate's own
        // "At least one Resources member must be defined" rule, which is a different
        // refusal and has its own suite.
        if case.template == "{}" {
            continue;
        }
        let (status, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateBody", &case.template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ValidateTemplate to answer a verdict on a well-formed \
             template whatever types it declares, got {status} with body {body}",
            case.case
        );
    }
}

/// The template a stack keeps when an update is refused.
const CATALOGUED_TEMPLATE: &str =
    r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"kept-topic"}}}}"#;

/// `UpdateStack` and `CreateChangeSet` refuse an unrecognised type too, and the stack keeps
/// the template it has.
#[tokio::test]
async fn every_operation_that_submits_a_template_refuses_an_unrecognised_type() {
    let router = router();
    let (created, created_body) =
        create_stack(&router, "update-target", CATALOGUED_TEMPLATE, &[]).await;
    assert_eq!(
        created, 200,
        "setup: expected the catalogued template to be accepted, got {created} with body \
         {created_body}"
    );
    // An update is refused outright while the create is still in progress, which would
    // hide the refusal under test: wait past the instant the create settles.
    wait_for_the_transition().await;

    let submitted = swf_domain_template();
    let operations: &[(&str, Vec<(String, String)>)] = &[
        (
            "UpdateStack",
            vec![
                member("StackName", "update-target"),
                member("TemplateBody", &submitted),
            ],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", "update-target"),
                member("ChangeSetName", "a-change-set"),
                member("TemplateBody", &submitted),
            ],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", "new-stack-from-change-set"),
                member("ChangeSetName", "a-create-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &submitted),
            ],
        ),
    ];
    for (action, members) in operations {
        let (status, body) = call(&router, action, members).await;
        assert_refused(action, status, &body, &["AWS::SWF::Domain"]);
    }

    // The stack still carries the template it was created with: a refused update changed
    // nothing.
    let (status, body) = get_template(&router, "update-target").await;
    assert_eq!(
        status, 200,
        "expected GetTemplate to answer for the stack, got {status} with body {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(CATALOGUED_TEMPLATE),
        "expected the refused update to leave the stack's template alone, got body {body}"
    );

    // ...and the same operations are accepted for a template whose types are catalogued.
    let accepted: &[(&str, Vec<(String, String)>)] = &[
        (
            "UpdateStack",
            vec![
                member("StackName", "update-target"),
                member(
                    "TemplateBody",
                    r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"next-topic"}}}}"#,
                ),
            ],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", "new-stack-from-change-set"),
                member("ChangeSetName", "a-create-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", CATALOGUED_TEMPLATE),
            ],
        ),
    ];
    for (action, members) in accepted {
        let (status, body) = call(&router, action, members).await;
        assert_eq!(
            status, 200,
            "case {action:?}: expected HTTP 200 for a catalogued template, got {status} \
             with body {body}"
        );
    }
}

/// The catalogue itself, as the rule depends on it: every published type is recognised,
/// every type this emulator deploys is published, a type with a suffix the catalogue does
/// not carry is not, and the namespace of the reported case has no entry at all.
#[test]
fn the_catalogue_recognises_what_it_publishes_and_nothing_else() {
    assert!(
        AWS_RESOURCE_TYPES.len() > IMPLEMENTED_RESOURCE_TYPES.len(),
        "expected the catalogue to be wider than the implemented set, got {} against {}",
        AWS_RESOURCE_TYPES.len(),
        IMPLEMENTED_RESOURCE_TYPES.len()
    );
    for resource_type in AWS_RESOURCE_TYPES {
        assert!(
            is_recognized(resource_type),
            "type {resource_type:?}: expected a published type to be recognized, it is not"
        );
        // A name the catalogue does not carry is not recognised, however close it is to
        // one it does.
        let misspelled = format!("{resource_type}Nope");
        assert!(
            is_published(&misspelled) || !is_recognized(&misspelled),
            "type {misspelled:?}: expected an uncatalogued name to be unrecognized, it is \
             recognized"
        );
    }
    // Amazon Simple Workflow Service publishes no CloudFormation resource type: its
    // domains and workflow types are created only through its own API.
    let swf: Vec<&&str> = AWS_RESOURCE_TYPES
        .iter()
        .filter(|entry| entry.starts_with("AWS::SWF::"))
        .collect();
    assert!(
        swf.is_empty(),
        "expected no AWS::SWF:: resource type in the catalogue, found {swf:?}"
    );
}
