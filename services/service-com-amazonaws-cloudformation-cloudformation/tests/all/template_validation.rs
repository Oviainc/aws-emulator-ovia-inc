// SPDX-License-Identifier: CC0-1.0

//! Template validation, end to end over the awsQuery wire.
//!
//! The rule under test: a template whose own declarations CloudFormation cannot resolve is
//! not one it deploys, so every operation that SUBMITS a body refuses the request
//! synchronously with HTTP 400 `ValidationError` and writes NOTHING — rather than answering
//! a stack id and then deploying a stack whose resources carry nulls. A template it CAN
//! resolve is accepted and deployed, and the values its expressions name are evaluated.
//!
//! The rows span the class the rule generalises over, one per kind of declaration a
//! template can get wrong:
//!
//! * a body neither reader will read at all;
//! * a `Parameters` key that is no parameter name;
//! * a function no template may use, and the `AWS::LanguageExtensions` functions in a
//!   template that does not declare that transform;
//! * an intrinsic spelled with arguments it does not take (`Fn::FindInMap`, `Fn::GetAtt`,
//!   `Fn::If`, `Fn::Length`, and the boolean operators);
//! * a `Ref`, an `Fn::GetAtt` or an `Fn::Sub` variable naming something the template does
//!   not declare, in either the `Resources` or the `Outputs` section;
//! * a `Condition` naming a condition the template does not declare;
//! * a `DependsOn` naming a logical id the `Resources` section does not declare, in either
//!   spelling the attribute takes;
//! * an `Fn::FindInMap` whose lookup the `Mappings` section does not carry — unless the
//!   lookup names a `DefaultValue`;
//! * a dependency CYCLE among the declared resources.
//!
//! Beside the refusals, two tables pin the other half of the rule: the templates these
//! checks must NOT refuse (the same shapes, resolvable), and the two templates a request may
//! submit whose DEPLOYMENT then fails and rolls the stack back — a `Rules` assertion the
//! parameter values do not satisfy, and an `Fn::ForEach` the declared transform cannot apply
//! where it stands.

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
    xml_texts,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// A template that is well-formed and deploys, used as the stack an update is submitted
/// against.
const DEPLOYABLE: &str =
    r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"valid-topic"}}}}"#;

/// `Capabilities` acknowledging the transform a row declares. A request may acknowledge a
/// capability its template does not need, so every row carries it.
fn acknowledged() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND")]
}

/// One template a submitting operation must refuse, and the text its refusal must carry.
struct Refused {
    case: &'static str,
    template: &'static str,
    /// Text the refusal's `Message` must contain: the part of the service's wording that
    /// names what is wrong, so the row pins the message and not just the status.
    message: &'static str,
}

/// The table: every kind of declaration a template can get wrong.
fn refused() -> Vec<Refused> {
    vec![
        Refused {
            case: "a body that is neither JSON nor YAML",
            template: "\tResources: [unclosed\n  - : :\n",
            message: "Template format error",
        },
        Refused {
            // The `Fn::ForEach` directive used as a parameter NAME is the same rule: a
            // parameter is named by its section key, and those names are alphanumeric.
            case: "a Fn::ForEach loop in the Parameters section",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Fn::ForEach::Params":["N",["a"],{"P${N}":{"Type":"String"}}]},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "Parameter name Fn::ForEach::Params is non alphanumeric.",
        },
        Refused {
            case: "a parameter name carrying a hyphen",
            template: r#"{"Parameters":{"not-a-name":{"Type":"String"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "Parameter name not-a-name is non alphanumeric.",
        },
        Refused {
            case: "a function no template may use",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::NoSuchIntrinsic":"anything"}}}}"#,
            message: "Encountered unsupported function: Fn::NoSuchIntrinsic",
        },
        Refused {
            case: "Fn::Length without the language-extensions transform",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Count":{"Value":{"Fn::Length":["a","b"]}}}}"#,
            message: "Encountered unsupported function: Fn::Length",
        },
        Refused {
            case: "Fn::ToJsonString without the language-extensions transform",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Rendered":{"Value":{"Fn::ToJsonString":{"a":1}}}}}"#,
            message: "Encountered unsupported function: Fn::ToJsonString",
        },
        Refused {
            case: "a dynamic Fn::GetAtt without the language-extensions transform",
            template: r#"{"Parameters":{"Which":{"Type":"String","Default":"Topic"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Arn":{"Value":{"Fn::GetAtt":[{"Ref":"Which"},"Arn"]}}}}"#,
            message: "every Fn::GetAtt object requires two non-empty parameters",
        },
        Refused {
            case: "an Fn::FindInMap with two keys instead of three",
            template: r#"{"Mappings":{"M":{"top":{"second":"v"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["M","top"]}}}}"#,
            message: "every Fn::FindInMap object requires three parameters",
        },
        Refused {
            case: "an Fn::If with two branches instead of three",
            template: r#"{"Conditions":{"Always":{"Fn::Equals":["a","a"]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::If":["Always","yes"]}}}}"#,
            message: "every Fn::If object requires three parameters",
        },
        Refused {
            case: "an Fn::Equals comparing three values",
            template: r#"{"Conditions":{"Bad":{"Fn::Equals":["a","a","a"]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "every Fn::Equals object requires a list of two values",
        },
        Refused {
            case: "an Fn::Not over two conditions",
            template: r#"{"Conditions":{"A":{"Fn::Equals":["a","a"]},"Bad":{"Fn::Not":[{"Condition":"A"},{"Condition":"A"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "every Fn::Not object requires a list of one condition",
        },
        Refused {
            case: "an Fn::And over a single condition",
            template: r#"{"Conditions":{"A":{"Fn::Equals":["a","a"]},"Bad":{"Fn::And":[{"Condition":"A"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "every Fn::And object requires a list of 2 to 10 conditions",
        },
        Refused {
            case: "an Fn::Or over eleven conditions",
            template: r#"{"Conditions":{"A":{"Fn::Equals":["a","a"]},"Bad":{"Fn::Or":[{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"},{"Condition":"A"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            message: "every Fn::Or object requires a list of 2 to 10 conditions",
        },
        Refused {
            case: "an Fn::Length over something that can be no list",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Count":{"Value":{"Fn::Length":"not-a-list"}}}}"#,
            message: "every Fn::Length object requires a list",
        },
        Refused {
            case: "a Ref in the Resources section naming nothing the template declares",
            template: r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"NoSuchThing"}}}}}"#,
            message: "Unresolved resource dependencies [NoSuchThing] in the Resources block",
        },
        Refused {
            case: "a Ref in the Outputs section naming nothing the template declares",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Ref":"NoSuchThing"}}}}"#,
            message: "Unresolved resource dependencies [NoSuchThing] in the Outputs block",
        },
        Refused {
            case: "an Fn::GetAtt naming a resource the template does not declare",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::GetAtt":["Absent","Arn"]}}}}"#,
            message: "Template error: instance of Fn::GetAtt references undefined resource Absent",
        },
        Refused {
            case: "an Fn::Sub variable neither its own map nor the template supplies",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::Sub":"${Unknown}-suffix"}}}}"#,
            message: "Unresolved resource dependencies [Unknown] in the Outputs block",
        },
        Refused {
            case: "a resource Condition naming a condition the template does not declare",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Condition":"NoSuchCondition"}}}"#,
            message: "Condition NoSuchCondition is not defined.",
        },
        Refused {
            case: "an output Condition naming a condition the template does not declare",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Condition":"NoSuchCondition","Value":"v"}}}"#,
            message: "Condition NoSuchCondition is not defined.",
        },
        Refused {
            case: "an Fn::If naming a condition the template does not declare",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::If":["NoSuchCondition","yes","no"]}}}}"#,
            message: "Condition NoSuchCondition is not defined.",
        },
        Refused {
            // "DependsOn: String | List of String": the attribute names RESOURCES, so a
            // name no resource is declared under is the same unresolved dependency of the
            // `Resources` section a `Ref` to an undeclared name is.
            case: "a DependsOn naming a logical id the template does not declare",
            template: r#"{"Resources":{"Bad":{"Type":"AWS::CloudFormation::WaitConditionHandle","DependsOn":"DoesNotExist"}}}"#,
            message: "Unresolved resource dependencies [DoesNotExist] in the Resources block",
        },
        Refused {
            case: "a DependsOn LIST one of whose entries the template does not declare",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"},"Bad":{"Type":"AWS::CloudFormation::WaitConditionHandle","DependsOn":["Topic","Ghost"]}}}"#,
            message: "Unresolved resource dependencies [Ghost] in the Resources block",
        },
        Refused {
            // A name the template DOES declare, but as a parameter: `DependsOn` states a
            // creation order over resources, and a parameter is not one of them.
            case: "a DependsOn naming a parameter instead of a resource",
            template: r#"{"Parameters":{"Stage":{"Type":"String","Default":"prod"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic","DependsOn":"Stage"}}}"#,
            message: "Unresolved resource dependencies [Stage] in the Resources block",
        },
        Refused {
            // A lookup into a map the `Mappings` section does not declare is diagnosed for
            // the MISSING MAP — the section is where its author has to add it — rather than
            // for the lookup path it took.
            case: "an Fn::FindInMap whose map the Mappings section does not carry",
            template: r#"{"Mappings":{"M":{"top":{"second":"v"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["Other","top","second"]}}}}"#,
            message: "Mapping named 'Other' is not present in the 'Mappings' section of template.",
        },
        Refused {
            // A template declaring NO `Mappings` section at all declares no map either, so a
            // lookup is refused for the map it named there too.
            case: "an Fn::FindInMap in a template declaring no Mappings section",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["Absent","top","second"]}}}}"#,
            message:
                "Mapping named 'Absent' is not present in the 'Mappings' section of template.",
        },
        Refused {
            case: "an Fn::FindInMap whose top-level key the map does not carry",
            template: r#"{"Mappings":{"M":{"top":{"second":"v"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["M","absent","second"]}}}}"#,
            message: "Unable to get mapping for M::absent::second",
        },
        Refused {
            case: "an Fn::FindInMap whose second-level key the entry does not carry",
            template: r#"{"Mappings":{"M":{"top":{"second":"v"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["M","top","absent"]}}}}"#,
            message: "Unable to get mapping for M::top::absent",
        },
        Refused {
            // The key a NESTED lookup supplies is resolved, so the outer lookup is held to
            // the key that one really answers.
            case: "an Fn::FindInMap whose nested lookup supplies a key the map does not carry",
            template: r#"{"Mappings":{"EnvTiers":{"prod":{"Tier":"large"}},"Sizes":{"small":{"Type":"t3.micro"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["Sizes",{"Fn::FindInMap":["EnvTiers","prod","Tier"]},"Type"]}}}}"#,
            message: "Unable to get mapping for Sizes::large::Type",
        },
        Refused {
            // A key a PARAMETER supplies is resolved from the values in force, which is
            // what the deployment would resolve it from.
            case: "an Fn::FindInMap whose parameter-supplied key the map does not carry",
            template: r#"{"Parameters":{"Env":{"Type":"String","Default":"staging"}},"Mappings":{"EnvTiers":{"prod":{"Tier":"large"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["EnvTiers",{"Ref":"Env"},"Tier"]}}}}"#,
            message: "Unable to get mapping for EnvTiers::staging::Tier",
        },
        Refused {
            // The cycle's members are named in the order the resource index they are keyed
            // by yields them, which here runs against the way the two ids sort.
            case: "two resources that Ref each other",
            template: r#"{"Resources":{"ParamA":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"ParamB"}}},"ParamB":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"ParamA"}}}}}"#,
            message: "Circular dependency between resources: [ParamB, ParamA]",
        },
        Refused {
            case: "three resources in a cycle through DependsOn and Fn::GetAtt",
            template: r#"{"Resources":{"First":{"Type":"AWS::SNS::Topic","DependsOn":"Second"},"Second":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::GetAtt":["Third","Arn"]}}},"Third":{"Type":"AWS::SNS::Topic","DependsOn":"First"}}}"#,
            message: "Circular dependency between resources: [Second, Third, First]",
        },
    ]
}

/// The refusal a response carries, or the whole body when it is no refusal.
///
/// Shared with the suites that assert the WORDING of these refusals
/// (`template_refusal_wording.rs`), so one rendering of a refusal serves both.
pub(crate) fn refusal(body: &str) -> String {
    match (xml_text(body, "Code"), xml_text(body, "Message")) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        _ => body.to_string(),
    }
}

#[tokio::test]
async fn a_template_cloudformation_cannot_resolve_is_refused_by_create_stack() {
    let router = router();

    for row in refused() {
        let (status_code, body) =
            create_stack(&router, "refused-stack", row.template, &acknowledged()).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: CreateStack must refuse the template, got {status_code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected the ValidationError refusal, got {}",
            row.case,
            refusal(&body)
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains(row.message),
            "case {:?}: expected a refusal naming {:?}, got {message:?}",
            row.case,
            row.message
        );
        // The refusal left NO stack behind: the name is still free, and nothing is
        // reported for it.
        let (status_code, described) = describe_stack(&router, "refused-stack").await;
        assert_eq!(
            status_code, 400,
            "case {:?}: a refused create must leave no stack, got {status_code} {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_same_template_is_refused_by_update_stack_and_create_change_set() {
    let router = router();
    // The stack an update is submitted against, deployed from a template that resolves.
    let (status_code, created) = create_stack(&router, "updatable", DEPLOYABLE, &[]).await;
    assert_eq!(
        status_code, 200,
        "the stack under test must be created, got {status_code} {created}"
    );
    wait_for_the_transition().await;

    for row in refused() {
        let members = [
            member("StackName", "updatable"),
            member("TemplateBody", row.template),
            member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND"),
        ];
        let (status_code, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: UpdateStack must refuse the template, got {status_code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected UpdateStack's ValidationError, got {}",
            row.case,
            refusal(&body)
        );

        let members = [
            member("StackName", "planned"),
            member("ChangeSetName", "cs"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", row.template),
            member("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND"),
        ];
        let (status_code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: CreateChangeSet must refuse the template, got {status_code} {body}",
            row.case
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {:?}: expected CreateChangeSet's ValidationError, got {}",
            row.case,
            refusal(&body)
        );
    }

    // The refused updates changed nothing: the stack still reports the template it was
    // created with, in the status its create left it in.
    let (_, described) = describe_stack(&router, "updatable").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "a refused update must leave the stack as it was, got {described}"
    );
}

/// One template the rules must NOT refuse, with an output it must report.
struct Accepted {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    /// The `OutputKey` to read back, and the `OutputValue` it must carry.
    output: (&'static str, &'static str),
}

/// The other half of the rule: the same shapes, resolvable, which deploy and whose values
/// are evaluated.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "a mapping lookup the Mappings section carries",
            stack_name: "map-hit",
            template: r#"{"Mappings":{"M":{"top":{"second":"found"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["M","top","second"]}}}}"#,
            output: ("V", "found"),
        },
        Accepted {
            case: "a nested mapping lookup whose outer key the inner one supplies",
            stack_name: "map-nested",
            template: r#"{"Mappings":{"EnvTiers":{"prod":{"Tier":"large"}},"Sizes":{"large":{"Type":"m5.large"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["Sizes",{"Fn::FindInMap":["EnvTiers","prod","Tier"]},"Type"]}}}}"#,
            output: ("V", "m5.large"),
        },
        Accepted {
            // "DefaultValue: The value to be returned if the key is not found": a lookup
            // that misses is the default, and no transform is needed to say so.
            case: "a mapping lookup that misses but names a DefaultValue",
            stack_name: "map-default",
            template: r#"{"Mappings":{"M":{"top":{"second":"found"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["M","absent","second",{"DefaultValue":"fallback"}]}}}}"#,
            output: ("V", "fallback"),
        },
        Accepted {
            case: "a parameter-supplied mapping key the map does carry",
            stack_name: "map-parameter",
            template: r#"{"Parameters":{"Env":{"Type":"String","Default":"prod"}},"Mappings":{"EnvTiers":{"prod":{"Tier":"large"}}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::FindInMap":["EnvTiers",{"Ref":"Env"},"Tier"]}}}}"#,
            output: ("V", "large"),
        },
        Accepted {
            case: "Fn::Length over a list-typed parameter, with the transform declared",
            stack_name: "length-parameter",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Names":{"Type":"CommaDelimitedList","Default":"a,b,c"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::Length":{"Ref":"Names"}}}}}"#,
            output: ("V", "3"),
        },
        Accepted {
            case: "Fn::ToJsonString over a document, with the transform declared",
            stack_name: "tojson",
            template: r#"{"Transform":"AWS::LanguageExtensions","Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::ToJsonString":{"b":2,"a":1}}}}}"#,
            output: ("V", r#"{"a":1,"b":2}"#),
        },
        Accepted {
            case: "a dynamic Fn::GetAtt, with the transform declared",
            stack_name: "dynamic-getatt",
            template: r#"{"Transform":"AWS::LanguageExtensions","Parameters":{"Which":{"Type":"String","Default":"Param"}},"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{"Name":"dynamic-probe","Type":"String","Value":"v"}}},"Outputs":{"V":{"Value":{"Fn::GetAtt":[{"Ref":"Which"},"Name"]}}}}"#,
            output: ("V", "dynamic-probe"),
        },
        Accepted {
            case: "an Fn::Sub reading a variable its own map supplies",
            stack_name: "sub-local",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"V":{"Value":{"Fn::Sub":["${Local}-x",{"Local":"local"}]}}}}"#,
            output: ("V", "local-x"),
        },
        Accepted {
            // A DAG is not a cycle: one resource depending on another is the ordinary case
            // the cycle check must not refuse.
            case: "a resource that depends on another, which depends on a third",
            stack_name: "acyclic",
            template: r#"{"Resources":{"First":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"first-topic"}},"Second":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"First"}}},"Third":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Second"}},"DependsOn":"Second"}},"Outputs":{"V":{"Value":{"Fn::GetAtt":["First","TopicName"]}}}}"#,
            output: ("V", "first-topic"),
        },
        Accepted {
            // The other spelling of the attribute, over TWO declared resources: a
            // `DependsOn` every entry of which names a declared resource states an order
            // the template can be deployed in.
            case: "a DependsOn list naming two resources the template declares",
            stack_name: "depends-on-list",
            template: r#"{"Resources":{"First":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"ordered-topic"}},"Second":{"Type":"AWS::SSM::Parameter","Properties":{"Name":"ordered-probe","Type":"String","Value":"v"}},"Last":{"Type":"AWS::CloudFormation::WaitConditionHandle","DependsOn":["First","Second"]}},"Outputs":{"V":{"Value":{"Fn::GetAtt":["Second","Name"]}}}}"#,
            output: ("V", "ordered-probe"),
        },
        Accepted {
            case: "a condition the template declares, gating an output",
            stack_name: "conditional",
            template: r#"{"Parameters":{"Stage":{"Type":"String","Default":"prod"}},"Conditions":{"IsProd":{"Fn::Equals":[{"Ref":"Stage"},"prod"]},"Both":{"Fn::And":[{"Condition":"IsProd"},{"Fn::Not":[{"Fn::Equals":[{"Ref":"Stage"},"test"]}]}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Condition":"IsProd"}},"Outputs":{"V":{"Condition":"Both","Value":{"Fn::If":["IsProd","yes","no"]}}}}"#,
            output: ("V", "yes"),
        },
    ]
}

#[tokio::test]
async fn a_template_cloudformation_can_resolve_is_deployed_and_its_values_evaluated() {
    let router = router();
    let cases = accepted();

    for case in &cases {
        let (status_code, body) =
            create_stack(&router, case.stack_name, case.template, &acknowledged()).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must accept the template, got {status_code} {body}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (_, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have completed, got {described}",
            case.case
        );
        // Read back through the stack itself: the value is one the deployment COMPUTED
        // from the template, not request input echoed back.
        let (key, value) = case.output;
        let keys = xml_texts(&described, "OutputKey");
        let values = xml_texts(&described, "OutputValue");
        let reported = keys
            .iter()
            .position(|reported| reported == key)
            .and_then(|index| values.get(index));
        assert_eq!(
            reported.map(String::as_str),
            Some(value),
            "case {:?}: output {key:?} must carry {value:?}, got keys {keys:?} values \
             {values:?}",
            case.case
        );
    }
}

/// One template a request may submit whose DEPLOYMENT then fails, and the text the stack's
/// own events must carry for it.
struct RolledBack {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    reason: &'static str,
}

#[tokio::test]
async fn a_deployment_the_template_fails_rolls_the_stack_back() {
    let router = router();
    let rows = vec![
        RolledBack {
            // "Rules: validates a parameter or a combination of parameters passed to a
            // template during a stack creation."
            case: "a failing Rules assertion",
            stack_name: "rule-failed",
            template: r#"{"Parameters":{"Env":{"Type":"String","Default":"prod"},"Size":{"Type":"String","Default":"small"}},"Rules":{"ProdRequiresLarge":{"RuleCondition":{"Fn::Equals":[{"Ref":"Env"},"prod"]},"Assertions":[{"Assert":{"Fn::Equals":[{"Ref":"Size"},"large"]},"AssertDescription":"prod stacks must use the large size"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            reason: "Parameter validation failed: assertion error: prod stacks must use the \
                     large size",
        },
        RolledBack {
            case: "a Fn::ForEach loop in a section the transform does not define it for",
            stack_name: "foreach-mappings",
            template: r#"{"Transform":"AWS::LanguageExtensions","Mappings":{"Fn::ForEach::Envs":["E",["a"],{"M${E}":{"k":{"v":"x"}}}]},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#,
            reason: "Transform AWS::LanguageExtensions failed with: Error parsing the template.",
        },
    ];

    for row in &rows {
        let (status_code, body) =
            create_stack(&router, row.stack_name, row.template, &acknowledged()).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: the create is accepted, got {status_code} {body}",
            row.case
        );
    }
    // The rollback the failed create owes is due one transition after the failure itself.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    for row in &rows {
        let (_, described) = describe_stack(&router, row.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {:?}: a failed deployment must roll the create back, got {described}",
            row.case
        );
        let events = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack_name)],
        )
        .await
        .1;
        let reasons = xml_texts(&events, "ResourceStatusReason");
        assert!(
            reasons.iter().any(|reason| reason.contains(row.reason)),
            "case {:?}: expected an event naming {:?}, got {reasons:?}",
            row.case,
            row.reason
        );
        // Nothing was provisioned for a deployment that never ran.
        assert!(
            xml_texts(&described, "Outputs").is_empty(),
            "case {:?}: a rolled-back create reports no outputs, got {described}",
            row.case
        );
    }
}

/// A rule whose lists are longer than any list this service resolves is ANSWERED, not
/// evaluated: the lists a `Fn::EachMemberIn` compares come from request-supplied parameter
/// values, so both lengths are the caller's to choose, and the assertion is skipped past the
/// bound rather than compared member by member.
#[tokio::test]
async fn a_rule_over_lists_past_the_resolved_bound_is_answered() {
    let router = router();
    // Far past MAX_RESOLVED_LIST_LEN, in both lists, from values a request supplies.
    let long: String = (0..4_000).map(|index| format!("v{index},")).collect();
    let template = r#"{"Parameters":{"Left":{"Type":"CommaDelimitedList","Default":"a"},"Right":{"Type":"CommaDelimitedList","Default":"a"}},"Rules":{"Subset":{"Assertions":[{"Assert":{"Fn::EachMemberIn":[{"Ref":"Left"},{"Ref":"Right"}]},"AssertDescription":"every left value must be a right value"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"bounded-topic"}}}}"#;

    let (status_code, body) = create_stack(
        &router,
        "bounded-rule",
        template,
        &[
            member("Parameters.member.1.ParameterKey", "Left"),
            member("Parameters.member.1.ParameterValue", &long),
            member("Parameters.member.2.ParameterKey", "Right"),
            member("Parameters.member.2.ParameterValue", &long),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "a rule over lists past the bound must be answered, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "bounded-rule").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "an assertion this service does not judge must not fail the create, got {described}"
    );
}
/// deploys, which is what makes the row above a property of the ASSERTION rather than of
/// the section.
#[tokio::test]
async fn a_rule_the_parameter_values_satisfy_leaves_the_create_alone() {
    let router = router();
    let template = r#"{"Parameters":{"Env":{"Type":"String","Default":"prod"},"Size":{"Type":"String","Default":"large"}},"Rules":{"ProdRequiresLarge":{"RuleCondition":{"Fn::Equals":[{"Ref":"Env"},"prod"]},"Assertions":[{"Assert":{"Fn::Contains":[["large","xlarge"],{"Ref":"Size"}]},"AssertDescription":"prod stacks must use a large size"}]}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"ruled-topic"}}}}"#;

    let (status_code, body) = create_stack(&router, "rule-passed", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "a satisfied rule must not stop the create, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "rule-passed").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "a satisfied rule leaves the create alone, got {described}"
    );

    // The same rule, with a value its assertion does NOT admit: the one difference between
    // the two stacks is the parameter value, so what fails is the assertion.
    let (status_code, body) = create_stack(
        &router,
        "rule-refused",
        template,
        &[
            member("Parameters.member.1.ParameterKey", "Size"),
            member("Parameters.member.1.ParameterValue", "small"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the create carrying the failing value is still accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "rule-refused").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::ROLLBACK_COMPLETE),
        "the value the assertion does not admit must roll the create back, got {described}"
    );
}
