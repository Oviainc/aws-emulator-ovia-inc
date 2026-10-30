// SPDX-License-Identifier: CC0-1.0

//! The two forms a packaged AWS SAM template is written in, end to end over HTTP.
//!
//! The rule under test, asserted over a SET of templates rather than one recorded body: a
//! publish carries "the raw packaged AWS SAM template of your application"
//! (`smithy_query(query="doc", shape="CreateApplicationVersionRequest",
//! member="TemplateBody")`), a packaged SAM template is a CloudFormation template, and
//! CloudFormation accepts one in EITHER of two forms — JSON, or YAML with the short-form
//! intrinsic tags (`!Ref`, `!Sub`, …). So every member this service DERIVES from a supplied
//! template — the SAM `Transform` it requires, the `ParameterDefinitions` the version
//! reports (`__listOfParameterDefinition`: `Name` and `ReferencedByResources` required,
//! plus `Type`, `DefaultValue`, `Description`, `Allowed*`/`Min*`/`Max*`, `NoEcho`), and the
//! `RequiredCapabilities` a deployment of it must acknowledge — is a function of the
//! document, not of the form it was written in: the two spellings of one document derive
//! the same members, and a YAML template declaring no SAM `Transform` is refused exactly as
//! its JSON spelling is.
//!
//! The harness (`Repo`, `Answer`) is the `applications` suite's, so these cases drive the
//! same wire path a `serverlessrepo` SDK client does.

use serde_json::{json, Value};

use super::applications::{minimal, Repo};
use super::version_templates::{assert_template_refusal, listed_names, listed_versions};

/// One row of the table: a case name, one document in BOTH spellings, and the two member
/// lists it derives.
struct Spellings {
    case: &'static str,
    json_body: String,
    yaml_body: &'static str,
    parameter_definitions: Value,
    required_capabilities: Value,
}

/// The document class the rule generalises over, each row written twice: as JSON, and as
/// the YAML a SAM CLI or console publish sends.
///
/// The rows differ along what the rule generalises over — how a parameter is referenced
/// (`!Ref`, `!Sub` as a string and `!Sub` with bindings, a parameter nothing references),
/// the parameter's type and which of the modelled constraint members it carries, the
/// scalar forms a YAML value takes (a plain number, a quoted number that stays text, a
/// boolean, a block list, a flow mapping), how many resources reference one parameter, and
/// which capabilities the declared resource types require.
fn spellings() -> Vec<Spellings> {
    vec![
        Spellings {
            case: "a Number parameter with a default, referenced by !Ref",
            json_body: json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Parameters": { "QueueVisibility": { "Type": "Number", "Default": 30 } },
                "Resources": {
                    "Q": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": { "VisibilityTimeout": { "Ref": "QueueVisibility" } }
                    }
                }
            })
            .to_string(),
            yaml_body: "Transform: AWS::Serverless-2016-10-31\n\
                        Parameters:\n  \
                          QueueVisibility:\n    \
                            Type: Number\n    \
                            Default: 30\n\
                        Resources:\n  \
                          Q:\n    \
                            Type: AWS::SQS::Queue\n    \
                            Properties:\n      \
                              VisibilityTimeout: !Ref QueueVisibility\n",
            parameter_definitions: json!([{
                "name": "QueueVisibility",
                "type": "Number",
                "defaultValue": "30",
                "referencedByResources": ["Q"]
            }]),
            required_capabilities: json!([]),
        },
        Spellings {
            case: "a String parameter two resources interpolate with Fn::Sub",
            json_body: json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Parameters": {
                    "Prefix": {
                        "Type": "String",
                        "Description": "the name prefix",
                        "AllowedValues": ["alpha", "beta"],
                        "AllowedPattern": "[a-z]+",
                        "MinLength": 1,
                        "MaxLength": 16,
                        "ConstraintDescription": "lowercase letters only",
                        "NoEcho": true
                    }
                },
                "Resources": {
                    "Writer": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": { "QueueName": { "Fn::Sub": "${Prefix}-writer" } }
                    },
                    "Reader": {
                        "Type": "AWS::SQS::Queue",
                        "Properties": {
                            "QueueName": {
                                "Fn::Sub": ["${Prefix}-${Suffix}", { "Suffix": "reader" }]
                            }
                        }
                    }
                }
            })
            .to_string(),
            yaml_body: "Transform: AWS::Serverless-2016-10-31\n\
                        Parameters:\n  \
                          Prefix:\n    \
                            Type: String\n    \
                            Description: the name prefix\n    \
                            AllowedValues:\n      \
                              - alpha\n      \
                              - beta\n    \
                            AllowedPattern: \"[a-z]+\"\n    \
                            MinLength: 1\n    \
                            MaxLength: 16\n    \
                            ConstraintDescription: lowercase letters only\n    \
                            NoEcho: true\n\
                        Resources:\n  \
                          Writer:\n    \
                            Type: AWS::SQS::Queue\n    \
                            Properties:\n      \
                              QueueName: !Sub \"${Prefix}-writer\"\n  \
                          Reader:\n    \
                            Type: AWS::SQS::Queue\n    \
                            Properties:\n      \
                              QueueName: !Sub\n        \
                                - \"${Prefix}-${Suffix}\"\n        \
                                - Suffix: reader\n",
            parameter_definitions: json!([{
                "name": "Prefix",
                "type": "String",
                "description": "the name prefix",
                "allowedValues": ["alpha", "beta"],
                "allowedPattern": "[a-z]+",
                "minLength": 1,
                "maxLength": 16,
                "constraintDescription": "lowercase letters only",
                "noEcho": true,
                "referencedByResources": ["Reader", "Writer"]
            }]),
            required_capabilities: json!([]),
        },
        Spellings {
            case: "a quoted number stays text, and a parameter nothing references lists \
                   no resource",
            json_body: json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Parameters": {
                    "Retries": { "Type": "Number", "Default": "007", "MinValue": 0, "MaxValue": 10 },
                    "Unused": { "Type": "CommaDelimitedList" }
                },
                "Resources": {
                    "Fn": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Environment": { "Variables": { "RETRIES": { "Ref": "Retries" } } }
                        }
                    }
                }
            })
            .to_string(),
            // The same document in YAML's FLOW style, which is how a packaged template
            // written by a tool often reaches the service.
            yaml_body: "Transform: AWS::Serverless-2016-10-31\n\
                        Parameters:\n  \
                          Retries: {Type: Number, Default: \"007\", MinValue: 0, MaxValue: 10}\n  \
                          Unused: {Type: CommaDelimitedList}\n\
                        Resources:\n  \
                          Fn:\n    \
                            Type: AWS::Serverless::Function\n    \
                            Properties:\n      \
                              Environment:\n        \
                                Variables: {RETRIES: !Ref Retries}\n",
            parameter_definitions: json!([
                {
                    "name": "Retries",
                    "type": "Number",
                    "defaultValue": "007",
                    "minValue": 0,
                    "maxValue": 10,
                    "referencedByResources": ["Fn"]
                },
                {
                    "name": "Unused",
                    "type": "CommaDelimitedList",
                    "referencedByResources": []
                }
            ]),
            required_capabilities: json!([]),
        },
        Spellings {
            case: "a template with no Parameters, whose resource types require \
                   acknowledgement",
            json_body: json!({
                "Transform": ["AWS::Serverless-2016-10-31"],
                "Resources": {
                    "Role": {
                        "Type": "AWS::IAM::Role",
                        "Properties": { "RoleName": "pinned-role" }
                    },
                    "Nested": { "Type": "AWS::Serverless::Application" }
                }
            })
            .to_string(),
            yaml_body: "Transform:\n  \
                          - AWS::Serverless-2016-10-31\n\
                        Resources:\n  \
                          Role:\n    \
                            Type: AWS::IAM::Role\n    \
                            Properties:\n      \
                              RoleName: pinned-role\n  \
                          Nested:\n    \
                            Type: AWS::Serverless::Application\n",
            parameter_definitions: json!([]),
            required_capabilities: json!(["CAPABILITY_AUTO_EXPAND", "CAPABILITY_NAMED_IAM"]),
        },
    ]
}

/// THE RULE: the two spellings of one template derive the same members, and those members
/// are the document's own.
#[tokio::test]
async fn the_two_spellings_of_one_template_derive_the_same_members() {
    let repo = Repo::new();

    for (index, row) in spellings().iter().enumerate() {
        let case = row.case;
        let semantic_version = format!("{}.0.0", index + 1);
        let mut answers: Vec<(&str, Value)> = Vec::new();

        for (form, body) in [
            ("JSON", row.json_body.clone()),
            ("YAML", row.yaml_body.to_string()),
        ] {
            // One application per form, so the two publishes cannot observe each other.
            let name = format!("spelling-{form}-{index}");
            let created = repo.create_application(minimal(&name)).await;
            assert_eq!(
                created.status, 201,
                "case {case:?} ({form}): creating {name:?} expected 201, got {} with {}",
                created.status, created.body
            );
            let application_id = created
                .at("applicationId")
                .as_str()
                .unwrap_or_default()
                .to_string();

            let published = repo
                .create_version(
                    &application_id,
                    &semantic_version,
                    json!({ "templateBody": body }),
                )
                .await;
            assert_eq!(
                published.status, 201,
                "case {case:?} ({form}): expected 201, got {} with {}",
                published.status, published.body
            );
            assert_eq!(
                published.at("parameterDefinitions"),
                &row.parameter_definitions,
                "case {case:?} ({form}): expected the parameters the template declares \
                 {}, got {}",
                row.parameter_definitions,
                published.at("parameterDefinitions")
            );
            assert_eq!(
                published.at("requiredCapabilities"),
                &row.required_capabilities,
                "case {case:?} ({form}): expected the capabilities its resources require \
                 {}, got {}",
                row.required_capabilities,
                published.at("requiredCapabilities")
            );

            // Round trip: the derived list is READ BACK from the store, not recomputed
            // per call.
            let fetched = repo
                .get_application(&application_id, Some(&semantic_version))
                .await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} ({form}): reading the version back expected 200, got {} \
                 with {}",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.at("version").get("parameterDefinitions"),
                Some(&row.parameter_definitions),
                "case {case:?} ({form}): GetApplication must report the stored parameters, \
                 got {}",
                fetched.body
            );
            assert_eq!(
                fetched.at("version").get("requiredCapabilities"),
                Some(&row.required_capabilities),
                "case {case:?} ({form}): GetApplication must report the stored \
                 capabilities, got {}",
                fetched.body
            );
            answers.push((form, published.body.clone()));
        }

        let (left_form, left) = &answers[0];
        let (right_form, right) = &answers[1];
        for member in ["parameterDefinitions", "requiredCapabilities"] {
            assert_eq!(
                left.get(member),
                right.get(member),
                "case {case:?}: the {left_form} and {right_form} spellings of one document \
                 must derive the same {member}, got {:?} and {:?}",
                left.get(member),
                right.get(member)
            );
        }
    }
}

/// THE RULE on the refusal's own wording: the 400 names the document it rejected, and what
/// it names is the PARSED document — so the two spellings of one template are quoted
/// identically, each value in the form the refusal renders it (an ordered mapping, a list,
/// text in quotes, a number as written, a boolean as `True`/`False`).
///
/// The rows differ along the value forms the rendering has to cover: a document of text
/// only, and one carrying a number, a boolean and a list.
#[tokio::test]
async fn the_refusal_quotes_the_parsed_document_whichever_form_it_arrived_in() {
    let repo = Repo::new();

    // (case, JSON spelling, YAML spelling, the document as the refusal quotes it)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "a document of text only",
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": { "Q": { "Type": "AWS::SQS::Queue" } }
            })
            .to_string(),
            "AWSTemplateFormatVersion: '2010-09-09'\n\
             Resources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            "OrderedDict({'AWSTemplateFormatVersion': '2010-09-09', 'Resources': \
             OrderedDict({'Q': OrderedDict({'Type': 'AWS::SQS::Queue'})})})",
        ),
        (
            "a document carrying a number, a boolean and a list",
            json!({
                "Parameters": {
                    "P": {
                        "Type": "Number",
                        "Default": 30,
                        "NoEcho": false,
                        "AllowedValues": [1, 2]
                    }
                }
            })
            .to_string(),
            "Parameters:\n  \
               P:\n    \
                 Type: Number\n    \
                 Default: 30\n    \
                 NoEcho: false\n    \
                 AllowedValues:\n      \
                   - 1\n      \
                   - 2\n",
            "OrderedDict({'Parameters': OrderedDict({'P': OrderedDict({'AllowedValues': \
             [1, 2], 'Default': 30, 'NoEcho': False, 'Type': 'Number'})})})",
        ),
    ];

    for (index, (case, json_body, yaml_body, echoed)) in rows.iter().enumerate() {
        let created = repo
            .create_application(minimal(&format!("refusal-echo-{index}")))
            .await;
        let application_id = created
            .at("applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = format!(
            "Resource with name [template] is invalid. Resource must have Transform field: \
             {echoed}"
        );

        for (form, body) in [("JSON", json_body.clone()), ("YAML", (*yaml_body).to_string())] {
            let answer = repo
                .create_version(
                    &application_id,
                    &format!("{}.0.0", index + 1),
                    json!({ "templateBody": body }),
                )
                .await;
            assert_template_refusal(case, &answer, None);
            let message = answer.at("message").as_str().unwrap_or_default().to_string();
            assert_eq!(
                message, expected,
                "case {case:?} ({form}): expected the refusal to quote the parsed document \
                 as {expected:?}, got {message:?}"
            );
        }
    }
}

/// THE RULE, continued: a YAML template is held to the SAM `Transform` exactly as its JSON
/// spelling is — refused with the modelled `BadRequestException` (HTTP 400) and nothing
/// published — and the forms CloudFormation gives the field are all accepted.
///
/// The rows span the YAML forms that declare the transform (one name, a block list, a flow
/// list of several) and the ones that declare none (absent, an empty name, a null, a value
/// that is not a name), plus the body that is no template document at all and is therefore
/// stored unjudged.
#[tokio::test]
async fn a_yaml_template_is_held_to_the_sam_transform() {
    let repo = Repo::new();
    let created = repo.create_application(minimal("yaml-transform-app")).await;
    let application_id = created
        .at("applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, body, accepted?, a token of the document the refusal must quote)
    let cases: Vec<(&str, &str, bool, Option<&str>)> = vec![
        (
            "plain CloudFormation in YAML, with no Transform",
            "AWSTemplateFormatVersion: '2010-09-09'\n\
             Resources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            false,
            Some("AWS::SQS::Queue"),
        ),
        (
            "the same document with the single Transform line",
            "AWSTemplateFormatVersion: '2010-09-09'\n\
             Transform: AWS::Serverless-2016-10-31\n\
             Resources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            true,
            None,
        ),
        (
            "the transform as a block list",
            "Transform:\n  - AWS::Serverless-2016-10-31\n\
             Resources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            true,
            None,
        ),
        (
            "a flow list of transforms, SAM among them",
            "Transform: [AWS::Include, AWS::Serverless-2016-10-31]\n\
             Resources: {}\n",
            true,
            None,
        ),
        (
            "an empty transform name",
            "Transform: \"\"\nResources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            false,
            Some("Transform"),
        ),
        (
            "a null transform",
            "Transform: ~\nResources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            false,
            Some("Transform"),
        ),
        (
            "a transform that is not a name",
            "Transform: 7\nResources:\n  Q:\n    Type: AWS::SQS::Queue\n",
            false,
            Some("Transform"),
        ),
        (
            // Not a modelled rule but this crate's standing policy for a body it does not
            // read as a template document (the same one it applies to a template named
            // only by `TemplateUrl`, which is never fetched): it is stored as supplied and
            // nothing is derived from it or judged about it.
            "a body that is no template document at all, so nothing is judged or guessed",
            "not a template",
            true,
            None,
        ),
    ];

    for (index, (case, body, accepted, quoted)) in cases.iter().enumerate() {
        // One semantic version per row, so an accepted row can never be mistaken for a
        // refused one by conflicting with it.
        let semantic_version = format!("{}.0.0", index + 1);
        let answer = repo
            .create_version(
                &application_id,
                &semantic_version,
                json!({ "templateBody": body }),
            )
            .await;
        let versions = listed_versions(&repo, &application_id).await;

        if *accepted {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected 201, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                versions.contains(&semantic_version),
                "case {case:?}: a published version is listed, got {versions:?}"
            );
            continue;
        }

        assert_template_refusal(case, &answer, *quoted);
        assert!(
            !versions.contains(&semantic_version),
            "case {case:?}: a refused publish leaves no version behind, got {versions:?}"
        );
        let absent = repo
            .get_application(&application_id, Some(&semantic_version))
            .await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: the refused version is absent, got {} with {}",
            absent.status, absent.body
        );
    }
}

/// THE RULE on the CREATE path: a `CreateApplication` whose `SemanticVersion` publishes a
/// transform-less YAML template is refused the same way, leaves no application behind, and
/// leaves the name free for a later create.
#[tokio::test]
async fn a_transform_less_yaml_template_is_refused_on_the_create_path_too() {
    let repo = Repo::new();
    let name = "yaml-create-refusal";
    let transform_less = "AWSTemplateFormatVersion: '2010-09-09'\n\
                          Resources:\n  Q:\n    Type: AWS::SQS::Queue\n";
    let packaged = "Transform: AWS::Serverless-2016-10-31\n\
                    Parameters:\n  \
                      Visibility:\n    \
                        Type: Number\n    \
                        Default: 30\n\
                    Resources:\n  \
                      Q:\n    \
                        Type: AWS::SQS::Queue\n    \
                        Properties:\n      \
                          VisibilityTimeout: !Ref Visibility\n";

    let mut request = minimal(name);
    request["semanticVersion"] = json!("1.0.0");
    request["templateBody"] = json!(transform_less);
    let refused = repo.create_application(request.clone()).await;
    assert_template_refusal(
        "a transform-less YAML template on CreateApplication",
        &refused,
        Some("AWS::SQS::Queue"),
    );
    let names = listed_names(&repo).await;
    assert!(
        !names.contains(&name.to_string()),
        "a refused create leaves no application behind, got {names:?}"
    );

    // The name is still free, and the same create with the packaged template is served.
    request["templateBody"] = json!(packaged);
    let created = repo.create_application(request).await;
    assert_eq!(
        created.status, 201,
        "the name a refused create did not take must still be free, got {} with {}",
        created.status, created.body
    );
    assert_eq!(
        created.at("version").get("parameterDefinitions"),
        Some(&json!([{
            "name": "Visibility",
            "type": "Number",
            "defaultValue": "30",
            "referencedByResources": ["Q"]
        }])),
        "the created version reports the parameters its YAML template declares, got {}",
        created.body
    );
}

/// THE RULE, per version: each version's list is derived from THAT version's own template,
/// so publishing a later version never rewrites an earlier version's derived members.
#[tokio::test]
async fn each_version_keeps_the_members_its_own_template_declared() {
    let repo = Repo::new();
    let created = repo.create_application(minimal("per-version-app")).await;
    let application_id = created
        .at("applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let with_parameters = "Transform: AWS::Serverless-2016-10-31\n\
                           Parameters:\n  \
                             Visibility:\n    \
                               Type: Number\n    \
                               Default: 30\n\
                           Resources:\n  \
                             Q:\n    \
                               Type: AWS::SQS::Queue\n    \
                               Properties:\n      \
                                 VisibilityTimeout: !Ref Visibility\n";
    let without_parameters = "Transform: AWS::Serverless-2016-10-31\n\
                              Resources:\n  \
                                Role:\n    \
                                  Type: AWS::IAM::Role\n";

    let expected: Vec<(&str, &str, Value, Value)> = vec![
        (
            "1.0.0",
            with_parameters,
            json!([{
                "name": "Visibility",
                "type": "Number",
                "defaultValue": "30",
                "referencedByResources": ["Q"]
            }]),
            json!([]),
        ),
        (
            "2.0.0",
            without_parameters,
            json!([]),
            json!(["CAPABILITY_IAM"]),
        ),
    ];

    for (semantic_version, body, _parameters, _capabilities) in &expected {
        let answer = repo
            .create_version(
                &application_id,
                semantic_version,
                json!({ "templateBody": body }),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "publishing {semantic_version}: expected 201, got {} with {}",
            answer.status, answer.body
        );
    }

    // Read back AFTER the later publish: each version still reports its own template's
    // members.
    for (semantic_version, _body, parameters, capabilities) in &expected {
        let fetched = repo
            .get_application(&application_id, Some(semantic_version))
            .await;
        assert_eq!(
            fetched.at("version").get("parameterDefinitions"),
            Some(parameters),
            "case {semantic_version:?}: expected {parameters}, got {}",
            fetched.body
        );
        assert_eq!(
            fetched.at("version").get("requiredCapabilities"),
            Some(capabilities),
            "case {semantic_version:?}: expected {capabilities}, got {}",
            fetched.body
        );
    }
}
