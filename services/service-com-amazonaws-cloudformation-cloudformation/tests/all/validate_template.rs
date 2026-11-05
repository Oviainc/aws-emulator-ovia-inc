// SPDX-License-Identifier: CC0-1.0

//! `ValidateTemplate`, end to end over the awsQuery wire.
//!
//! The rule under test: `ValidateTemplate` answers a verdict COMPUTED from the template
//! body the request carries — CloudFormation reads that body as JSON, failing that as
//! YAML, and refuses it with HTTP 400 `ValidationError` when it is neither or when it
//! declares no resource — and reports what the body declares: one `TemplateParameter` per
//! parameter the `Parameters` section declares (its key, its scalar `Default`, its
//! `NoEcho` and its `Description`), the template's own `Description`, the IAM capability
//! the declared resources require with the reason naming the resource types that required
//! it, and the names the `Transform` section declares. It writes nothing.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling `ValidateTemplate` sees. The table spans the class the rule generalises
//! over: both template spellings (JSON and YAML, long-form and short-form intrinsics),
//! the count of declared parameters, each spelling of a declaration member (a string
//! default, a numeric one, `NoEcho` as a boolean and as text, a declaration with no
//! optional member at all), a template with no `Parameters` section at all, templates
//! declaring IAM resources with generated and with custom names, several entries of the
//! documented IAM resource catalogue, a non-IAM resource that carries a name property,
//! and each spelling of the `Transform` section (one name, a list of names, a macro
//! object).

use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    MAX_DESCRIPTION_BYTES, MAX_TEMPLATE_NODES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::TEMPLATE_BODY_MAX_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, list_members, member, outside_parameters, reason_resource_types, router,
    router_with_engine, store, xml_text, xml_unescape, CAPABILITIES_REASON_PREFIX,
};

/// One `TemplateParameter` as the response carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireParameter {
    key: Option<String>,
    default_value: Option<String>,
    /// Read as text, because that is what the XML envelope carries (`true`/`false`).
    no_echo: Option<String>,
    description: Option<String>,
}

/// A parameter row, spelled as briefly as a table row can be.
fn parameter(
    key: &str,
    default_value: Option<&str>,
    no_echo: bool,
    description: Option<&str>,
) -> WireParameter {
    WireParameter {
        key: Some(key.to_string()),
        default_value: default_value.map(ToString::to_string),
        no_echo: Some(no_echo.to_string()),
        description: description.map(ToString::to_string),
    }
}

/// The text between `<tag …>` and `</tag>`, matching the element by name so an opening
/// tag carrying attributes is still found.
fn element_body(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = xml.find(&open)?;
    let after_name = &xml[start + open.len()..];
    // The character after the name must end it, so `<member>` is not matched by a longer
    // element name that merely starts the same way.
    if !after_name.starts_with('>') && !after_name.starts_with(char::is_whitespace) {
        return None;
    }
    let body = &after_name[after_name.find('>')? + 1..];
    let end = body.find(&close)?;
    Some(body[..end].to_string())
}

/// One element's text inside a single member block.
fn element(entry: &str, tag: &str) -> Option<String> {
    element_body(entry, tag).map(|text| xml_unescape(&text))
}

/// The `Parameters` the response reports, member by member, so a parameter's key stays
/// paired with its own default (and a declaration carrying no `Default` is visible as
/// such instead of borrowing the next one's).
fn wire_parameters(xml: &str) -> Vec<WireParameter> {
    list_members(xml, "Parameters")
        .iter()
        .map(|entry| WireParameter {
            key: element(entry, "ParameterKey"),
            default_value: element(entry, "DefaultValue"),
            no_echo: element(entry, "NoEcho"),
            description: element(entry, "Description"),
        })
        .collect()
}

/// The `Capabilities` the response reports.
fn wire_capabilities(xml: &str) -> Vec<String> {
    list_members(xml, "Capabilities")
        .iter()
        .map(|entry| entry.trim().to_string())
        .collect()
}

/// The `DeclaredTransforms` the response reports.
fn wire_transforms(xml: &str) -> Vec<String> {
    list_members(xml, "DeclaredTransforms")
        .iter()
        .map(|entry| entry.trim().to_string())
        .collect()
}

async fn validate(router: &MultiServiceRouter, template_body: &str) -> (u16, String) {
    call(
        router,
        "ValidateTemplate",
        &[member("TemplateBody", template_body)],
    )
    .await
}

/// One row of the "what the body declares" table.
struct Case {
    case: &'static str,
    template: String,
    /// The `Description` the response must report.
    description: Option<&'static str>,
    /// Every `TemplateParameter`, in the order the response carries them.
    parameters: Vec<WireParameter>,
    /// The `Capabilities` the template requires: empty for a template that requires none.
    capabilities: Vec<&'static str>,
    /// The resource types `CapabilitiesReason` must name, sorted; empty when the response
    /// must carry no reason at all.
    reason_types: Vec<&'static str>,
    /// `DeclaredTransforms`, sorted the same way for the same reason.
    transforms: Vec<&'static str>,
}

/// The templates this table validates.
///
/// The IAM rows draw on "Acknowledging IAM resources in CloudFormation templates", which
/// lists the resource types requiring `CAPABILITY_IAM` and says a custom name for one of
/// them requires `CAPABILITY_NAMED_IAM` instead; `AWS::IAM::InstanceProfile`,
/// `AWS::IAM::ManagedPolicy`, `AWS::IAM::Policy` and `AWS::IAM::User` are catalogue
/// entries beyond the `AWS::IAM::Role` an IAM example usually shows.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "JSON template, one resource, no Parameters section",
            template: r#"{
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "A queue and nothing else",
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}
            }"#
            .to_string(),
            description: Some("A queue and nothing else"),
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: Vec::new(),
        },
        Case {
            case: "JSON template with no Description at all",
            template: r#"{"Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}}"#.to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: Vec::new(),
        },
        Case {
            case: "YAML template, short-form intrinsics, three declared parameters",
            template: "AWSTemplateFormatVersion: '2010-09-09'\n\
                 Description: A topic named by a parameter\n\
                 Parameters:\n\
                 \x20 AlphaSecret:\n\
                 \x20   Type: String\n\
                 \x20   NoEcho: true\n\
                 \x20 BetaName:\n\
                 \x20   Type: String\n\
                 \x20   Default: beta\n\
                 \x20   Description: The name the topic takes\n\
                 \x20 GammaCount:\n\
                 \x20   Type: Number\n\
                 \x20   Default: 7\n\
                 Resources:\n\
                 \x20 Topic:\n\
                 \x20   Type: AWS::SNS::Topic\n\
                 \x20   Properties:\n\
                 \x20     TopicName: !Ref BetaName\n\
                 Outputs:\n\
                 \x20 TopicArn:\n\
                 \x20   Value: !Ref Topic\n"
                .to_string(),
            description: Some("A topic named by a parameter"),
            parameters: vec![
                parameter("AlphaSecret", None, true, None),
                parameter("BetaName", Some("beta"), false, Some("The name the topic takes")),
                // A numeric default is reported as the text the template spelled.
                parameter("GammaCount", Some("7"), false, None),
            ],
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: Vec::new(),
        },
        Case {
            case: "JSON template whose NoEcho is spelled as text",
            template: r#"{
                "Parameters": {"Secret": {"Type": "String", "NoEcho": "true",
                                          "Description": "A masked value"}},
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}
            }"#
            .to_string(),
            description: None,
            parameters: vec![parameter("Secret", None, true, Some("A masked value"))],
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: Vec::new(),
        },
        Case {
            case: "an IAM role whose name CloudFormation generates",
            template: r#"{
                "Description": "A role",
                "Resources": {"Role": {"Type": "AWS::IAM::Role",
                                       "Properties": {"AssumeRolePolicyDocument": {}}}}
            }"#
            .to_string(),
            description: Some("A role"),
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_IAM"],
            reason_types: vec!["AWS::IAM::Role"],
            transforms: Vec::new(),
        },
        Case {
            case: "an IAM role carrying a custom RoleName",
            template: r#"{
                "Resources": {"Role": {"Type": "AWS::IAM::Role",
                                       "Properties": {"RoleName": "a-named-role"}}}
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
            reason_types: vec!["AWS::IAM::Role"],
            transforms: Vec::new(),
        },
        Case {
            case: "an IAM user whose custom name is built by an intrinsic function",
            template: "Resources:\n\
                 \x20 User:\n\
                 \x20   Type: AWS::IAM::User\n\
                 \x20   Properties:\n\
                 \x20     UserName: !Sub '${AWS::StackName}-user'\n"
                .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
            reason_types: vec!["AWS::IAM::User"],
            transforms: Vec::new(),
        },
        Case {
            case: "two catalogued IAM types, neither of them named",
            template: r#"{
                "Resources": {
                    "Profile": {"Type": "AWS::IAM::InstanceProfile",
                                "Properties": {"Roles": ["a-role"]}},
                    "Policy": {"Type": "AWS::IAM::Policy",
                               "Properties": {"PolicyName": "a-policy"}}
                }
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_IAM"],
            // `AWS::IAM::Policy` takes no custom NAME property — `PolicyName` is a
            // required property of the type, not a custom resource name — so the pair
            // requires the unnamed capability.
            reason_types: vec!["AWS::IAM::InstanceProfile", "AWS::IAM::Policy"],
            transforms: Vec::new(),
        },
        Case {
            case: "a catalogued IAM type with a custom name beside one without",
            template: r#"{
                "Resources": {
                    "Key": {"Type": "AWS::IAM::AccessKey", "Properties": {"UserName": "a-user"}},
                    "Policy": {"Type": "AWS::IAM::ManagedPolicy",
                               "Properties": {"ManagedPolicyName": "a-managed-policy"}}
                }
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
            reason_types: vec!["AWS::IAM::AccessKey", "AWS::IAM::ManagedPolicy"],
            transforms: Vec::new(),
        },
        Case {
            case: "a non-IAM resource that carries a role property",
            template: r#"{
                "Resources": {"Fn": {"Type": "AWS::Lambda::Function",
                                     "Properties": {"Role": "arn:aws:iam::000000000000:role/r",
                                                    "FunctionName": "a-function"}}}
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: Vec::new(),
        },
        Case {
            case: "a Transform section naming one macro",
            template: r#"{
                "Transform": "AWS::Serverless-2016-10-31",
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: vec!["AWS::Serverless-2016-10-31"],
        },
        Case {
            case: "a Transform section naming a list of macros, one as an object",
            template: r#"{
                "Transform": ["AWS::Include", {"Name": "MyMacro",
                                               "Parameters": {"Mode": "strict"}}],
                "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}
            }"#
            .to_string(),
            description: None,
            parameters: Vec::new(),
            capabilities: Vec::new(),
            reason_types: Vec::new(),
            transforms: vec!["AWS::Include", "MyMacro"],
        },
        Case {
            case: "a YAML Transform section beside IAM resources and parameters",
            template: "Transform: AWS::Serverless-2016-10-31\n\
                 Description: A serverless role\n\
                 Parameters:\n\
                 \x20 Stage:\n\
                 \x20   Type: String\n\
                 \x20   Default: dev\n\
                 Resources:\n\
                 \x20 Role:\n\
                 \x20   Type: AWS::IAM::Role\n\
                 \x20   Properties:\n\
                 \x20     RoleName: !Ref Stage\n"
                .to_string(),
            description: Some("A serverless role"),
            parameters: vec![parameter("Stage", Some("dev"), false, None)],
            capabilities: vec!["CAPABILITY_NAMED_IAM"],
            reason_types: vec!["AWS::IAM::Role"],
            transforms: vec!["AWS::Serverless-2016-10-31"],
        },
    ]
}

#[tokio::test]
async fn validate_template_reports_what_the_template_body_declares() {
    let router = router();
    for case in cases() {
        let label = case.case;
        let (status, body) = validate(&router, &case.template).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );

        assert_eq!(
            xml_text(&outside_parameters(&body), "Description").as_deref(),
            case.description,
            "case {label:?}: expected the template's own Description, got {body}"
        );
        assert_eq!(
            wire_parameters(&body),
            case.parameters,
            "case {label:?}: expected one TemplateParameter per declared parameter, got {body}"
        );
        assert_eq!(
            wire_capabilities(&body),
            case.capabilities,
            "case {label:?}: expected the capabilities the template requires, got {body}"
        );
        let reason = xml_text(&body, "CapabilitiesReason");
        if case.reason_types.is_empty() {
            assert!(
                reason.is_none(),
                "case {label:?}: expected no CapabilitiesReason, got {reason:?}"
            );
        } else {
            let reason = reason.unwrap_or_else(|| {
                panic!("case {label:?}: expected a CapabilitiesReason, got {body}")
            });
            assert!(
                reason.starts_with(CAPABILITIES_REASON_PREFIX) && reason.ends_with(']'),
                "case {label:?}: expected a reason of the form \
                 {CAPABILITIES_REASON_PREFIX}…], got {reason:?}"
            );
            assert_eq!(
                reason_resource_types(&reason),
                case.reason_types,
                "case {label:?}: expected the reason to name the resource types that \
                 required the capability, got {reason:?}"
            );
        }
        let mut transforms = wire_transforms(&body);
        transforms.sort();
        assert_eq!(
            transforms, case.transforms,
            "case {label:?}: expected the declared transforms, got {body}"
        );
    }
}

/// One row of the refusal table: a request `ValidateTemplate` must not answer a verdict
/// for, and the status, wire code and message the refusal carries.
struct RefusalCase {
    case: &'static str,
    members: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    /// Text the message must contain, so a row cannot pass by failing for another reason.
    message: String,
}

#[tokio::test]
async fn validate_template_refuses_a_body_it_cannot_report_on() {
    let over_long_body = format!(
        r#"{{"Description": "{}", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue"}}}}}}"#,
        "d".repeat(TEMPLATE_BODY_MAX_BYTES)
    );
    let over_long_description = format!(
        r#"{{"Description": "{}", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue"}}}}}}"#,
        "d".repeat(MAX_DESCRIPTION_BYTES + 1)
    );
    // Past the node bound the reader puts on a built tree, and well inside the body's
    // byte maximum: a FLAT body is bounded by node count, not by its nesting.
    let too_many_nodes = format!(
        r#"{{"Resources": {{"Q": {{"Type": "AWS::SQS::Queue", "Properties": {{"Tags": [{}]}}}}}}}}"#,
        vec!["0"; MAX_TEMPLATE_NODES + 1].join(",")
    );
    let refusals = vec![
        RefusalCase {
            case: "a body that is neither valid JSON nor valid YAML",
            members: vec![member("TemplateBody", "{\"Resources\": ")],
            status: 400,
            code: "ValidationError",
            message: "Template format error".to_string(),
        },
        RefusalCase {
            case: "a body whose YAML is a flow mapping that never closes",
            members: vec![member("TemplateBody", "Resources: {Queue: {Type: ")],
            status: 400,
            code: "ValidationError",
            message: "Template format error".to_string(),
        },
        RefusalCase {
            case: "a valid YAML scalar, which declares no section at all",
            members: vec![member("TemplateBody", "not-a-template")],
            status: 400,
            code: "ValidationError",
            message: "At least one Resources member must be defined".to_string(),
        },
        RefusalCase {
            case: "a template whose Resources section is empty",
            members: vec![member("TemplateBody", r#"{"Resources": {}}"#)],
            status: 400,
            code: "ValidationError",
            message: "At least one Resources member must be defined".to_string(),
        },
        RefusalCase {
            case: "a template whose only resource declares no Type",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {"Queue": {"Properties": {}}}}"#,
            )],
            status: 400,
            code: "ValidationError",
            // The entry is named by its own path: "Type: required" is a rule about THAT
            // entry, not about the section, so the two refusals differ.
            message: "[/Resources/Queue] Every Resources object must contain a Type member."
                .to_string(),
        },
        RefusalCase {
            case: "a template declaring one resource beside an untyped sibling",
            members: vec![member(
                "TemplateBody",
                r#"{"Resources": {
                    "Queue": {"Type": "AWS::SQS::Queue"},
                    "Untyped": {"Properties": {}}
                }}"#,
            )],
            status: 400,
            code: "ValidationError",
            message: "[/Resources/Untyped] Every Resources object must contain a Type member."
                .to_string(),
        },
        RefusalCase {
            case: "a body one byte past the documented TemplateBody maximum",
            members: vec![member("TemplateBody", &over_long_body)],
            status: 400,
            code: "ValidationError",
            message: "templateBody".to_string(),
        },
        RefusalCase {
            case: "a flat JSON body past the reader's node bound",
            members: vec![member("TemplateBody", &too_many_nodes)],
            status: 400,
            code: "ValidationError",
            message: "Template format error".to_string(),
        },
        RefusalCase {
            case: "a Description one byte past its documented maximum",
            members: vec![member("TemplateBody", &over_long_description)],
            status: 400,
            code: "ValidationError",
            // The bound comes from the one place that carries it, so a row cannot
            // re-state a literal the handler also spells.
            message: format!("Description must be no longer than {MAX_DESCRIPTION_BYTES} bytes"),
        },
        RefusalCase {
            case: "neither TemplateBody nor TemplateURL",
            members: Vec::new(),
            status: 400,
            code: "ValidationError",
            message: "Either TemplateBody or TemplateURL must be specified".to_string(),
        },
        RefusalCase {
            // A `TemplateURL` is resolved by reading the object it addresses out of this
            // emulator's own Amazon S3, so one naming a bucket nothing created is the
            // REQUEST's own bad reference — not a verdict on a template nobody read.
            case: "a TemplateURL naming no object this account can read",
            members: vec![member(
                "TemplateURL",
                "https://s3.amazonaws.com/a-bucket/a-template.json",
            )],
            status: 400,
            code: "ValidationError",
            message: "S3 error: The specified bucket does not exist\nFor more information \
                      check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html"
                .to_string(),
        },
    ];

    let router = router();
    for refusal in &refusals {
        let label = refusal.case;
        let (status, body) = call(&router, "ValidateTemplate", &refusal.members).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected {} , got {status} {body}",
            refusal.status
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(refusal.code),
            "case {label:?}: expected the wire code {:?}, got {body}",
            refusal.code
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains(refusal.message.as_str()),
            "case {label:?}: expected a message naming {:?}, got {message:?}",
            refusal.message
        );
    }
}

#[tokio::test]
async fn a_flat_body_just_under_the_node_bound_is_still_read() {
    let router = router();
    // The sibling of the node-bound refusal row: the SAME shape, a dozen nodes under the
    // bound instead of past it, so what separates the two rows is the node COUNT and not
    // the body being malformed. Both are far inside the byte maximum.
    let body = format!(
        r#"{{"Description": "a flat body", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue", "Properties": {{"Tags": [{}]}}}}}}}}"#,
        vec!["0"; MAX_TEMPLATE_NODES - 12].join(",")
    );
    let (status, response) = validate(&router, &body).await;
    assert_eq!(
        status, 200,
        "case \"a flat body just under the node bound\": expected 200, got {status}"
    );
    assert_eq!(
        xml_text(&outside_parameters(&response), "Description").as_deref(),
        Some("a flat body"),
        "case \"a flat body just under the node bound\": expected the body to have been \
         read, got {response}"
    );
}

#[tokio::test]
async fn a_description_at_its_documented_bound_is_reported() {
    let router = router();
    // The published bound itself: a Description of exactly the documented maximum is
    // valid and reported in full (one byte past it is the refusal row above).
    let description = "d".repeat(MAX_DESCRIPTION_BYTES);
    let template = format!(
        r#"{{"Description": "{description}", "Resources": {{"Q": {{"Type": "AWS::SQS::Queue"}}}}}}"#
    );
    let (status, body) = validate(&router, &template).await;
    assert_eq!(
        status, 200,
        "case \"a Description at its documented maximum\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&outside_parameters(&body), "Description"),
        Some(description),
        "case \"a Description at its documented maximum\": expected the whole Description \
         back, got {body}"
    );
}

#[tokio::test]
async fn template_body_wins_when_both_are_passed() {
    let router = router();
    // "Conditional: You must pass TemplateURL or TemplateBody. If both are passed, only
    // TemplateBody is used."
    let (status, body) = call(
        &router,
        "ValidateTemplate",
        &[
            member(
                "TemplateBody",
                r#"{"Description": "The body that is used",
                    "Resources": {"Q": {"Type": "AWS::SQS::Queue"}}}"#,
            ),
            member("TemplateURL", "https://s3.amazonaws.com/a-bucket/other.json"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"both TemplateBody and TemplateURL\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&outside_parameters(&body), "Description").as_deref(),
        Some("The body that is used"),
        "case \"both TemplateBody and TemplateURL\": expected the body's own Description, \
         got {body}"
    );
}

#[tokio::test]
async fn a_lookup_a_parameter_value_would_decide_is_not_refused() {
    // The template-validation rules `ValidateTemplate` applies are the ones decided from the
    // DOCUMENT; a request carrying only a body fixes no parameter value, so an expression
    // whose resolution a supplied value would decide — here an `Fn::FindInMap` whose key a
    // parameter with no `Default` supplies — is one this verdict has nothing to say about,
    // and the template is reported on rather than refused. The same template IS refused by
    // the operations that submit it with a value the map does not carry
    // (`template_validation.rs`), which is where the value comes from.
    let router = router();
    let template = r#"{
        "Parameters": {"Env": {"Type": "String"}},
        "Mappings": {"EnvTiers": {"prod": {"Tier": "large"}}},
        "Resources": {"Topic": {"Type": "AWS::SNS::Topic"}},
        "Outputs": {"Tier": {"Value": {"Fn::FindInMap": ["EnvTiers", {"Ref": "Env"}, "Tier"]}}}
    }"#;
    let (status, body) = validate(&router, template).await;
    assert_eq!(
        status, 200,
        "case \"a lookup keyed on a parameter with no Default\": expected the template to be \
         reported on, got {status} {body}"
    );
    assert_eq!(
        wire_parameters(&body),
        vec![parameter("Env", None, false, None)],
        "case \"a lookup keyed on a parameter with no Default\": expected the declared \
         parameter to be reported, got {body}"
    );
}

#[tokio::test]
async fn validating_a_template_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let template = r#"{
        "Description": "A validated template",
        "Parameters": {"Name": {"Type": "String", "Default": "a-name"}},
        "Resources": {"Role": {"Type": "AWS::IAM::Role", "Properties": {"RoleName": "r"}}}
    }"#;
    let (first_status, first) = validate(&router, template).await;
    let (second_status, second) = validate(&router, template).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case \"validate twice\": expected both calls to answer 200, got \
         {first_status} and {second_status}"
    );
    assert_eq!(
        wire_parameters(&first),
        wire_parameters(&second),
        "case \"validate twice\": expected the same parameters both times, got \
         {first} and {second}"
    );
    assert_eq!(
        wire_capabilities(&first),
        wire_capabilities(&second),
        "case \"validate twice\": expected the same capabilities both times, got \
         {first} and {second}"
    );
    let held = store(&engine);
    let state = held.lock();
    assert!(
        state.stacks.is_empty() && state.change_sets.is_empty(),
        "case \"validate twice\": expected a validation to persist nothing, got \
         {} stack(s) and {} change set(s)",
        state.stacks.len(),
        state.change_sets.len()
    );
}
