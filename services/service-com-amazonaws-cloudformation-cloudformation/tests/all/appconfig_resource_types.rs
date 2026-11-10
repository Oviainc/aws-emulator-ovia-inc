// SPDX-License-Identifier: CC0-1.0

//! The AWS AppConfig resource types a template may declare, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **an operation that submits a template accepts every resource type the
//! "AWS resource and property types reference" publishes for a service namespace** — the
//! emulator's catalogue in `state::resource_types` is a transcription of that published set,
//! so a published type is recognised, the stack reaches `CREATE_COMPLETE` and the resource is
//! recorded under the type the template declared — **while a name that reference does not
//! publish is still refused** with HTTP 400, `ValidationError` and `Template format error:
//! Unrecognized resource types: [<types>]`.
//!
//! `unrecognized_resource_types.rs` owns the REFUSING half of that rule across namespaces
//! and spellings, and `cognito_resource_types.rs` owns the admitting half for Amazon
//! Cognito. This suite is the admitting half over one whole published namespace: AWS
//! AppConfig (the `cloudFormationName` of its own model, `smithy_query(query =
//! "service_info")` on `com-amazonaws-appconfig-amazonappconfig.json`), the namespace of the
//! reported case — `AWS::AppConfig::ExperimentDefinition`, which the catalogue did not carry,
//! so every template declaring an AppConfig experiment was refused at submit time and no such
//! stack could be created.
//!
//! The rows differ along what the rule generalises over — the resource NAME inside one
//! published namespace. Every type the reference publishes under `AWS::AppConfig::` is a row,
//! which makes the reported type one row among its siblings rather than a special case, and
//! each row is crossed with the two things a published type owes a caller: the submission is
//! accepted, and the declared resource is readable back through `DescribeStackResources`
//! under its declared type. The refusing rows are the near misses of the same namespace — a
//! plural, a wrong case, an invented resource and the service model's own name as the
//! namespace — so admitting the published set is not admitting `AWS::AppConfig::<anything>`.
//!
//! The four types whose resources a stack really creates in AWS AppConfig
//! (`state::provisioning::appconfig_resources`) carry the identifier that service minted as
//! their physical resource id, which is what a `Ref` to one resolves to;
//! `appconfig_estate_provisioning.rs` is the suite for that half of the rule. Every OTHER
//! published type of the namespace is declared-only (`unprovisioned_resources.rs` pins that:
//! nothing is called in the owning service and the stack completes) and carries the physical
//! resource id CloudFormation's own naming rules give it. What `Fn::GetAtt` reads off the
//! declared-only types (an experiment definition's `Id`, a hosted configuration version's
//! `VersionNumber`) is their published RETURN VALUES, a separate question this suite asserts
//! nothing about: it belongs to the open stack-outputs issue
//! (`20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md`).

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
    xml_texts,
};
use crate::stack_resources::{resources_of, wire_resources};

/// The namespace AWS AppConfig's resource types are published under, which is the
/// `cloudFormationName` of its own service model.
const APPCONFIG_NAMESPACE: &str = "AWS::AppConfig::";

/// Every resource type the "AWS resource and property types reference" publishes under
/// `AWS::AppConfig::`, with the operation of the owning service — present in this checkout's
/// `appconfig` model, `smithy_query(query = "list_operations")` — that deploying one calls.
///
/// Source: that reference's AWS AppConfig section. This is the expectation side of the
/// catalogue: the test below holds `AWS_RESOURCE_TYPES`' own `AWS::AppConfig::` group to
/// exactly this set, so a missing entry (the reported defect) and a stray one both fail.
const PUBLISHED: &[&str] = &[
    // CreateApplication
    "AWS::AppConfig::Application",
    // CreateConfigurationProfile
    "AWS::AppConfig::ConfigurationProfile",
    // StartDeployment
    "AWS::AppConfig::Deployment",
    // CreateDeploymentStrategy
    "AWS::AppConfig::DeploymentStrategy",
    // CreateEnvironment
    "AWS::AppConfig::Environment",
    // CreateExperimentDefinition — the reported type.
    "AWS::AppConfig::ExperimentDefinition",
    // CreateExtension
    "AWS::AppConfig::Extension",
    // CreateExtensionAssociation
    "AWS::AppConfig::ExtensionAssociation",
    // CreateHostedConfigurationVersion
    "AWS::AppConfig::HostedConfigurationVersion",
];

/// The stack name a one-resource stack for `resource_type` is created under: the type's last
/// segment, lower-cased, which is inside a stack name's own alphabet.
fn stack_name_for(resource_type: &str) -> String {
    let resource = resource_type
        .rsplit("::")
        .next()
        .unwrap_or(resource_type)
        .to_ascii_lowercase();
    format!("appconfig-{resource}")
}

/// The four types of this namespace whose resources a stack really CREATES in AWS AppConfig
/// (`state::provisioning::appconfig_resources`): each one's physical resource id is the `Id`
/// that service minted — modelled `^[a-z0-9]{4,7}$` — rather than a name CloudFormation
/// generated, which `appconfig_estate_provisioning.rs` is the suite for. Every other
/// published type of the namespace is still declared-only.
const PROVISIONED: &[&str] = &[
    "AWS::AppConfig::Application",
    "AWS::AppConfig::ConfigurationProfile",
    "AWS::AppConfig::DeploymentStrategy",
    "AWS::AppConfig::Environment",
];

/// The logical id of the application a child of the namespace is declared under, for the rows
/// whose own create takes one.
const PARENT: &str = "Parent";

/// A one-resource template declaring `resource_type` under the logical id `Subject`.
///
/// A type this emulator leaves declared-only is declared with NO properties: a published type
/// may be declared with whatever properties its own reference gives it, and what this suite is
/// about is the TYPE. A type whose resource the stack really creates carries the properties
/// that create REQUIRES — and, where the create takes one, the application the subject belongs
/// to beside it — because a create the owning service could not make is reported as the
/// resource's failure rather than as a stack that completed. The reported template's own
/// properties are the separate case below.
fn one_resource_template(resource_type: &str) -> String {
    let mut resources = serde_json::Map::new();
    let subject = match resource_type {
        "AWS::AppConfig::Environment" => {
            resources.insert(PARENT.to_string(), parent_application());
            json!({
                "Type": resource_type,
                "Properties": { "ApplicationId": { "Ref": PARENT } }
            })
        }
        "AWS::AppConfig::ConfigurationProfile" => {
            resources.insert(PARENT.to_string(), parent_application());
            json!({
                "Type": resource_type,
                "Properties": {
                    "ApplicationId": { "Ref": PARENT },
                    "LocationUri": "hosted"
                }
            })
        }
        // `CreateDeploymentStrategy` requires both of these members
        // (`smithy_query(query="operation",operation="CreateDeploymentStrategy")`).
        "AWS::AppConfig::DeploymentStrategy" => json!({
            "Type": resource_type,
            "Properties": { "DeploymentDurationInMinutes": 0, "GrowthFactor": 100 }
        }),
        _ => json!({ "Type": resource_type }),
    };
    resources.insert("Subject".to_string(), subject);
    json!({ "Resources": Value::Object(resources) }).to_string()
}

/// The application a child of the namespace is declared under, named by CloudFormation itself.
fn parent_application() -> Value {
    json!({ "Type": "AWS::AppConfig::Application" })
}

/// The catalogue's own `AWS::AppConfig::` group, in catalogue order.
fn catalogued_appconfig_types() -> Vec<&'static str> {
    AWS_RESOURCE_TYPES
        .iter()
        .copied()
        .filter(|entry| entry.starts_with(APPCONFIG_NAMESPACE))
        .collect()
}

/// The catalogue carries exactly the published AWS AppConfig namespace — every type the
/// reference publishes, and nothing else under it.
#[test]
fn the_catalogue_carries_the_published_appconfig_namespace() {
    let mut expected: Vec<&str> = PUBLISHED.to_vec();
    expected.sort_unstable();
    let mut actual = catalogued_appconfig_types();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "case {APPCONFIG_NAMESPACE:?}: expected the catalogue to carry exactly the published \
         AWS AppConfig types {expected:?}, got {actual:?}"
    );
    for resource_type in PUBLISHED {
        assert!(
            is_recognized(resource_type),
            "case {resource_type:?}: expected a published type to be recognized, it is not"
        );
    }
}

/// `CreateStack` accepts a template declaring any published AWS AppConfig type, the stack
/// completes, and the resource is readable back under the type that was declared.
#[tokio::test]
async fn every_published_appconfig_type_may_be_declared_and_is_recorded() {
    let router = router();
    // Submit every row first, then wait once for the transition they all owe: the delay is
    // per stack and runs from its own create.
    for resource_type in PUBLISHED {
        let stack_name = stack_name_for(resource_type);
        let template = one_resource_template(resource_type);
        let (status_code, body) = create_stack(&router, &stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected HTTP 200 from CreateStack for a published \
             resource type, got {status_code} with body {body}"
        );
    }
    wait_for_the_transition().await;

    for resource_type in PUBLISHED {
        let stack_name = stack_name_for(resource_type);
        let (status_code, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected DescribeStacks to report the created stack, got \
             {status_code} with body {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {resource_type:?}: expected the stack to reach {:?}, got body {body}",
            status::CREATE_COMPLETE
        );

        let (status_code, body) = resources_of(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected DescribeStackResources to answer, got \
             {status_code} with body {body}"
        );
        let reported = wire_resources(&body);
        let subject = reported
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some("Subject"))
            .unwrap_or_else(|| {
                panic!(
                    "case {resource_type:?}: expected the declared subject to be recorded, got \
                     {reported:?}"
                )
            });
        assert_eq!(
            subject.resource_type.as_deref(),
            Some(*resource_type),
            "case {resource_type:?}: expected the declared type to be reported, got {:?}",
            subject.resource_type
        );
        let physical_id = subject.physical_id.clone().unwrap_or_default();
        if PROVISIONED.contains(resource_type) {
            // The resource really exists in AppConfig, so its physical id is the `Id` that
            // service minted rather than a name CloudFormation generated.
            assert!(
                (4..=7).contains(&physical_id.len())
                    && physical_id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "case {resource_type:?}: expected the id AppConfig mints (4-7 lowercase \
                 alphanumerics), got {physical_id:?}"
            );
        } else {
            assert!(
                physical_id.contains(&stack_name) && physical_id.contains("Subject"),
                "case {resource_type:?}: expected a generated physical id built from the stack \
                 name and the logical id, got {physical_id:?}"
            );
        }
    }
}

/// One refusing row: a name of the same namespace the reference does not publish.
struct NearMiss {
    case: &'static str,
    stack_name: &'static str,
    declared: &'static [&'static str],
}

/// The near misses of the admitted namespace. Admitting the published set must not admit
/// `AWS::AppConfig::<anything>`.
const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "the reported type, pluralised",
        stack_name: "appconfig-plural",
        declared: &["AWS::AppConfig::ExperimentDefinitions"],
    },
    NearMiss {
        case: "the reported type in the wrong case",
        stack_name: "appconfig-wrong-case",
        declared: &["AWS::AppConfig::experimentdefinition"],
    },
    NearMiss {
        case: "a resource the AWS AppConfig namespace does not publish",
        stack_name: "appconfig-invented",
        declared: &["AWS::AppConfig::Experiment"],
    },
    NearMiss {
        case: "the service model's own name as the namespace",
        stack_name: "appconfig-model-namespace",
        declared: &["AWS::AmazonAppConfig::ExperimentDefinition"],
    },
    NearMiss {
        case: "two unpublished names beside published ones, each named once",
        stack_name: "appconfig-mixed",
        declared: &[
            "AWS::AppConfig::ExperimentRun",
            "AWS::AppConfig::HostedConfiguration",
        ],
    },
];

/// A name the reference does not publish is refused, whatever namespace it wears, and
/// nothing is created for it.
#[tokio::test]
async fn an_unpublished_appconfig_name_is_still_refused() {
    let router = router();
    for near_miss in NEAR_MISSES {
        let NearMiss {
            case,
            stack_name,
            declared,
        } = near_miss;
        // A published type beside the unpublished ones: the refusal names only what it did
        // not recognise.
        let mut resources = serde_json::Map::new();
        resources.insert(
            "Application".to_string(),
            json!({ "Type": "AWS::AppConfig::Application" }),
        );
        for (index, resource_type) in declared.iter().enumerate() {
            resources.insert(format!("Unknown{index}"), json!({ "Type": resource_type }));
        }
        let template = Value::Object(
            [("Resources".to_string(), Value::Object(resources))]
                .into_iter()
                .collect(),
        )
        .to_string();

        let (status_code, body) = create_stack(&router, stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected HTTP 400 for an unpublished resource type, got \
             {status_code} with body {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the wire code ValidationError, got body {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        for resource_type in *declared {
            assert!(
                message.contains(resource_type),
                "case {case:?}: expected the refusal to name {resource_type:?}, got {message:?}"
            );
        }
        assert!(
            !message.contains("AWS::AppConfig::Application,")
                && !message.contains("[AWS::AppConfig::Application]"),
            "case {case:?}: expected the refusal not to name the published type beside them, \
             got {message:?}"
        );
        let (status_code, body) = describe_stack(&router, stack_name).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the refused stack not to exist, DescribeStacks answered \
             {status_code} with body {body}"
        );
    }
}

/// The reported estate: an AppConfig application, its environment, a configuration profile,
/// a hosted configuration version of that profile and the experiment definition over them,
/// declared together as the reported template does — with a stack parameter feeding the
/// application's name and each resource referencing the one above it.
fn experiment_estate_template() -> String {
    json!({
        "Parameters": {
            "ApplicationName": { "Type": "String", "Default": "an-application" }
        },
        "Resources": {
            "Application": {
                "Type": "AWS::AppConfig::Application",
                "Properties": { "Name": { "Ref": "ApplicationName" } }
            },
            "Environment": {
                "Type": "AWS::AppConfig::Environment",
                "Properties": {
                    "ApplicationId": { "Ref": "Application" },
                    "Name": "an-environment"
                }
            },
            "ConfigurationProfile": {
                "Type": "AWS::AppConfig::ConfigurationProfile",
                "Properties": {
                    "ApplicationId": { "Ref": "Application" },
                    "Name": "a-profile",
                    "LocationUri": "hosted"
                }
            },
            "HostedConfigurationVersion": {
                "Type": "AWS::AppConfig::HostedConfigurationVersion",
                "Properties": {
                    "ApplicationId": { "Ref": "Application" },
                    "ConfigurationProfileId": { "Ref": "ConfigurationProfile" },
                    "ContentType": "application/json",
                    "Content": "{\"enabled\": true}"
                }
            },
            "ExperimentDefinition": {
                "Type": "AWS::AppConfig::ExperimentDefinition",
                "Properties": {
                    "ApplicationId": { "Ref": "Application" },
                    "EnvironmentId": { "Ref": "Environment" },
                    "Name": "an-experiment",
                    "Description": "the reported experiment"
                }
            }
        },
        "Outputs": {
            "ApplicationRef": { "Value": { "Ref": "Application" } },
            "ExperimentRef": { "Value": { "Ref": "ExperimentDefinition" } }
        }
    })
    .to_string()
}

/// The reported template deploys: `CreateStack` accepts it, the stack completes, every
/// declared resource is recorded under its declared type, and each `Ref` output resolves to
/// the physical resource id the stack reports for the resource it names.
#[tokio::test]
async fn the_reported_experiment_estate_deploys() {
    let router = router();
    let case = "an application, environment, profile, hosted version and experiment definition";
    let stack_name = "appconfig-experiment-estate";
    let template = experiment_estate_template();

    let (status_code, body) = call(
        &router,
        "CreateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", &template),
            member("Parameters.member.1.ParameterKey", "ApplicationName"),
            member("Parameters.member.1.ParameterValue", "experiment-app"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status_code} with body {body}"
    );
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, stack_name).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} with body {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the stack to reach {:?}, got body {body}",
        status::CREATE_COMPLETE
    );

    let (resource_status, resource_body) = resources_of(&router, stack_name).await;
    assert_eq!(
        resource_status, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {resource_status} with \
         body {resource_body}"
    );
    let recorded = wire_resources(&resource_body);
    let mut recorded_types: Vec<String> = recorded
        .iter()
        .map(|resource| resource.resource_type.clone().unwrap_or_default())
        .collect();
    recorded_types.sort();
    assert_eq!(
        recorded_types,
        vec![
            "AWS::AppConfig::Application".to_string(),
            "AWS::AppConfig::ConfigurationProfile".to_string(),
            "AWS::AppConfig::Environment".to_string(),
            "AWS::AppConfig::ExperimentDefinition".to_string(),
            "AWS::AppConfig::HostedConfigurationVersion".to_string(),
        ],
        "case {case:?}: expected every declared resource to be recorded, got {recorded_types:?}"
    );

    // Each `Ref` output carries a VALUE — an `OutputKey` with no `OutputValue` beside it is
    // an output that resolved to nothing, which the counts are what catch — and that value is
    // the physical resource id the stack reports for the resource the output names.
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    assert_eq!(
        keys.len(),
        2,
        "case {case:?}: expected both declared outputs, got {keys:?} in {body}"
    );
    assert_eq!(
        values.len(),
        keys.len(),
        "case {case:?}: expected one OutputValue per OutputKey {keys:?}, got {values:?} in \
         {body}"
    );
    let physical_id_of = |logical_id: &str| -> String {
        recorded
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(logical_id))
            .and_then(|resource| resource.physical_id.clone())
            .unwrap_or_default()
    };
    let resolved: Vec<(String, String)> = keys.iter().cloned().zip(values.iter().cloned()).collect();
    for (key, value) in &resolved {
        let logical_id = match key.as_str() {
            "ApplicationRef" => "Application",
            "ExperimentRef" => "ExperimentDefinition",
            other => panic!("case {case:?}: unexpected output {other:?} in {resolved:?}"),
        };
        let expected = physical_id_of(logical_id);
        assert!(
            !expected.is_empty(),
            "case {case:?}: expected a physical resource id for {logical_id:?}, got none in \
             {recorded:?}"
        );
        assert_eq!(
            value, &expected,
            "case {case:?}: expected output {key:?} to resolve to {logical_id:?}'s physical \
             resource id {expected:?}, got {value:?}"
        );
    }

    // The same template is accepted by the other operations that submit one for deployment,
    // which are held to the same catalogue.
    let (status_code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "appconfig-experiment-change-set"),
            member("ChangeSetName", "a-change-set"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateChangeSet to accept the same template, got \
         {status_code} with body {body}"
    );
    // UpdateStack refuses a template identical to the stack's own ("No updates are to be
    // performed."), so the submitted body declares one more resource — of a published type of
    // the same namespace, which is what this asserts the operation admits.
    let mut updated: Value = serde_json::from_str(&template).expect("the template is JSON");
    updated["Resources"]["Extension"] = json!({
        "Type": "AWS::AppConfig::Extension",
        "Properties": { "Name": "an-extension" }
    });
    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", &updated.to_string()),
            member("Parameters.member.1.ParameterKey", "ApplicationName"),
            member("Parameters.member.1.ParameterValue", "experiment-app"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected UpdateStack to accept a template declaring the same published \
         types, got {status_code} with body {body}"
    );
}
