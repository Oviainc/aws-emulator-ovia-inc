// SPDX-License-Identifier: CC0-1.0

//! `GetGeneratedTemplate`, end to end over the awsQuery wire: the TEMPLATE a generated
//! template record produced, in the language the read asks for.
//!
//! RULE: `GetGeneratedTemplate` answers, for the record its `GeneratedTemplateName`
//! addresses in the account and Region the request is signed for, the template of that
//! record's last generation to reach a complete status — one entry per covered resource,
//! keyed by the logical id that resource has in the final template, declaring the
//! resource's own type, the `DeletionPolicy` and `UpdateReplacePolicy` the record's
//! `TemplateConfiguration` assigns to the template's resources and the properties the
//! record identifies the resource by — rendered in the `Format` the request named and in
//! JSON when it named none; the empty template while no generation of the record has
//! completed yet; and an address no record of that Region holds is refused 404
//! `GeneratedTemplateNotFound`.
//!
//! Four suites, one per face of that rule, each a table whose rows differ along what the
//! rule generalises over rather than along one recorded case:
//!
//! 1. **The rendered template.** How many resources a generation covers (none, one, two and
//!    three), where each came from (a base stack, the request's own list, and both), whether
//!    the record carries a template configuration, whether a listed resource was given a
//!    logical id at all, and both languages the model's `TemplateFormat` offers plus a read
//!    that names none. Each row's body is read back with this service's OWN template
//!    reader (`state::template::parse`, the reader every operation that takes a
//!    `TemplateBody` uses), so what is asserted is that the emulator answered a template a
//!    CloudFormation client can deploy, not that it answered a particular string.
//! 2. **Which generation's template.** The empty template while the record's first
//!    generation is still in flight, the generation's own template once the clock has made
//!    its conclusion due.
//! 3. **One template, either language.** The JSON and the YAML spelling of one record
//!    declare exactly the same resources, properties and attributes: the language is the
//!    read's choice, the template is the record's.
//! 4. **Addressing, and that the read reports.** By name and by the record's own ARN, a
//!    name nothing created, a record of another Region, and two reads in a row answering
//!    the same body and leaving the record's status where it was.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use service_com_amazonaws_cloudformation_cloudformation::state::template;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    create_generated_template, deploy, describe_generated_template, error_code,
    wait_for_the_generation,
};
use crate::stack_lifecycle::{call, call_in, member, router, xml_text, REGION};

/// A base stack declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring three resources of two types.
const THREE_RESOURCE_STACK: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Beta":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// `GetGeneratedTemplate` for one address, in one language (or in none, so the service's
/// own default answers).
async fn get_generated_template(
    router: &MultiServiceRouter,
    name_or_id: &str,
    format: Option<&str>,
) -> (u16, String) {
    get_generated_template_in(router, REGION, name_or_id, format).await
}

/// [`get_generated_template`], signed for `region`: a generated template belongs to the
/// Region the call that created it was signed for, so the credential scope of the read is
/// what decides which Region's records it can address.
async fn get_generated_template_in(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
    format: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("GeneratedTemplateName", name_or_id)];
    if let Some(format) = format {
        members.push(member("Format", format));
    }
    call_in(router, region, "GetGeneratedTemplate", &members).await
}

/// One resource a request lists itself, as `ResourceDefinition` models it: the type, the
/// logical id it asks for (which the member makes OPTIONAL) and the one property pair that
/// identifies it.
struct Listed {
    resource_type: &'static str,
    logical_resource_id: Option<&'static str>,
    identifier: (&'static str, &'static str),
}

/// The request members that submit one `Resources` entry, in the awsQuery flattened form.
fn listed_members(position: usize, listed: &Listed) -> Vec<(String, String)> {
    let index = position + 1;
    let mut members = vec![
        member(
            &format!("Resources.member.{index}.ResourceType"),
            listed.resource_type,
        ),
        member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.1.key"),
            listed.identifier.0,
        ),
        member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.1.value"),
            listed.identifier.1,
        ),
    ];
    if let Some(logical_resource_id) = listed.logical_resource_id {
        members.push(member(
            &format!("Resources.member.{index}.LogicalResourceId"),
            logical_resource_id,
        ));
    }
    members
}

/// One resource the rendered template must declare: its logical id, its type and the
/// properties it declares.
type ExpectedResource = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// One row of the rendering table.
struct BodyCase {
    case: &'static str,
    /// The base stack's template, or `None` for a request that names no base stack.
    template: Option<&'static str>,
    listed: &'static [Listed],
    /// `TemplateConfiguration`'s `DeletionPolicy` and `UpdateReplacePolicy`, as the
    /// modelled enums spell them (`DELETE | RETAIN`), when the request supplies them.
    configuration: Option<(&'static str, &'static str)>,
    /// The `Format` the read names, or `None` for a read that names none.
    format: Option<&'static str>,
    expected: &'static [ExpectedResource],
}

/// The resource-attribute spelling of a configured policy.
///
/// The configuration is modelled as `DELETE | RETAIN`, while the `DeletionPolicy` and
/// `UpdateReplacePolicy` resource attributes of a template take that attribute's own
/// documented values — `Delete` and `Retain` — which is what the rendered template has to
/// carry for a client to deploy it.
fn attribute_policy(case: &str, configured: &str) -> &'static str {
    match configured {
        "DELETE" => "Delete",
        "RETAIN" => "Retain",
        other => panic!("case {case:?}: no attribute spelling for the policy {other:?}"),
    }
}

/// The rows: every count of covered resources the rule spans, every origin of one, with and
/// without a configuration, with and without a logical id of the request's own, in both
/// languages and in none.
const BODY_CASES: &[BodyCase] = &[
    BodyCase {
        case: "one base-stack resource, no configuration, a read naming no format",
        template: Some(ONE_RESOURCE_STACK),
        listed: &[],
        configuration: None,
        format: None,
        expected: &[("Topic", "AWS::SNS::Topic", &[])],
    },
    BodyCase {
        case: "three base-stack resources of two types, retained, JSON",
        template: Some(THREE_RESOURCE_STACK),
        listed: &[],
        configuration: Some(("RETAIN", "RETAIN")),
        format: Some("JSON"),
        expected: &[
            ("Alpha", "AWS::SNS::Topic", &[]),
            ("Beta", "AWS::SNS::Topic", &[]),
            ("Bucket", "AWS::S3::Bucket", &[]),
        ],
    },
    BodyCase {
        case: "two listed resources with their own logical ids, deleted on delete, YAML",
        template: None,
        listed: &[
            Listed {
                resource_type: "AWS::SNS::Topic",
                logical_resource_id: Some("ListedTopic"),
                identifier: (
                    "TopicArn",
                    "arn:aws:sns:us-east-1:000000000000:a-listed-topic",
                ),
            },
            Listed {
                resource_type: "AWS::DynamoDB::Table",
                logical_resource_id: Some("ListedTable"),
                identifier: ("TableName", "a-listed-table"),
            },
        ],
        configuration: Some(("DELETE", "RETAIN")),
        format: Some("YAML"),
        expected: &[
            (
                "ListedTable",
                "AWS::DynamoDB::Table",
                &[("TableName", "a-listed-table")],
            ),
            (
                "ListedTopic",
                "AWS::SNS::Topic",
                &[(
                    "TopicArn",
                    "arn:aws:sns:us-east-1:000000000000:a-listed-topic",
                )],
            ),
        ],
    },
    BodyCase {
        case: "a listed resource the request gave no logical id, YAML",
        template: None,
        listed: &[Listed {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: None,
            identifier: ("BucketName", "a-nameless-bucket"),
        }],
        configuration: None,
        format: Some("YAML"),
        // No logical id was asked for, so the template keys the resource by one derived
        // from its own type: the type's segments past the vendor prefix.
        expected: &[(
            "S3Bucket",
            "AWS::S3::Bucket",
            &[("BucketName", "a-nameless-bucket")],
        )],
    },
    BodyCase {
        case: "a base stack plus a listed resource of its own, JSON",
        template: Some(ONE_RESOURCE_STACK),
        listed: &[Listed {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: Some("ListedBucket"),
            identifier: ("BucketName", "a-listed-bucket"),
        }],
        configuration: Some(("RETAIN", "DELETE")),
        format: Some("JSON"),
        expected: &[
            (
                "ListedBucket",
                "AWS::S3::Bucket",
                &[("BucketName", "a-listed-bucket")],
            ),
            ("Topic", "AWS::SNS::Topic", &[]),
        ],
    },
    BodyCase {
        case: "two listed resources the request named the same logical id",
        template: None,
        listed: &[
            Listed {
                resource_type: "AWS::SNS::Topic",
                logical_resource_id: Some("Twin"),
                identifier: ("TopicArn", "arn:aws:sns:us-east-1:000000000000:a-twin-topic"),
            },
            Listed {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: Some("Twin"),
                identifier: ("BucketName", "a-twin-bucket"),
            },
        ],
        configuration: None,
        format: Some("JSON"),
        // A template keys its resources by logical id, so two resources cannot share one:
        // the first keeps the id asked for and the next free ordinal keys the second.
        expected: &[
            (
                "Twin",
                "AWS::SNS::Topic",
                &[("TopicArn", "arn:aws:sns:us-east-1:000000000000:a-twin-topic")],
            ),
            (
                "Twin2",
                "AWS::S3::Bucket",
                &[("BucketName", "a-twin-bucket")],
            ),
        ],
    },
    BodyCase {
        case: "two listed resources of one type, neither given a logical id, YAML",
        template: None,
        listed: &[
            Listed {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: None,
                identifier: ("BucketName", "the-first-bucket"),
            },
            Listed {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: None,
                identifier: ("BucketName", "the-second-bucket"),
            },
        ],
        configuration: None,
        format: Some("YAML"),
        expected: &[
            (
                "S3Bucket",
                "AWS::S3::Bucket",
                &[("BucketName", "the-first-bucket")],
            ),
            (
                "S3Bucket2",
                "AWS::S3::Bucket",
                &[("BucketName", "the-second-bucket")],
            ),
        ],
    },
    BodyCase {
        case: "a derived logical id yielding to the one a later resource was named",
        template: None,
        listed: &[
            Listed {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: None,
                identifier: ("BucketName", "the-unnamed-bucket"),
            },
            Listed {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: Some("S3Bucket"),
                identifier: ("BucketName", "the-named-bucket"),
            },
        ],
        configuration: None,
        format: Some("JSON"),
        // The id a request asked for is never taken from it by one derived for another
        // resource, wherever in the list that resource sits.
        expected: &[
            (
                "S3Bucket",
                "AWS::S3::Bucket",
                &[("BucketName", "the-named-bucket")],
            ),
            (
                "S3Bucket2",
                "AWS::S3::Bucket",
                &[("BucketName", "the-unnamed-bucket")],
            ),
        ],
    },
    BodyCase {
        case: "a generation covering no resource at all, JSON",
        template: None,
        listed: &[],
        configuration: None,
        format: Some("JSON"),
        expected: &[],
    },
    BodyCase {
        case: "a generation covering no resource at all, YAML",
        template: None,
        listed: &[],
        configuration: None,
        format: Some("YAML"),
        expected: &[],
    },
];

/// The request members for one row's create.
fn create_members(row: &BodyCase, stack_name: Option<&str>) -> Vec<(String, String)> {
    let mut members: Vec<(String, String)> = Vec::new();
    if let Some(stack_name) = stack_name {
        members.push(member("StackName", stack_name));
    }
    for (position, listed) in row.listed.iter().enumerate() {
        members.extend(listed_members(position, listed));
    }
    if let Some((deletion_policy, update_replace_policy)) = row.configuration {
        members.push(member(
            "TemplateConfiguration.DeletionPolicy",
            deletion_policy,
        ));
        members.push(member(
            "TemplateConfiguration.UpdateReplacePolicy",
            update_replace_policy,
        ));
    }
    members
}

/// The `TemplateBody` of a 200 `GetGeneratedTemplate`, with the status it reported.
fn answered_template(case: &str, status: u16, body: &str) -> (String, String) {
    assert_eq!(
        status, 200,
        "case {case:?}: GetGeneratedTemplate answered {status} with {body}"
    );
    let reported = xml_text(body, "Status")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Status, got {body}"));
    let template_body = xml_text(body, "TemplateBody")
        .unwrap_or_else(|| panic!("case {case:?}: expected a TemplateBody, got {body}"));
    // `TemplateBody` is `@length(min: 1)`: a body is always answered, even for a template
    // that declares nothing.
    assert!(
        !template_body.is_empty(),
        "case {case:?}: expected a non-empty template body, got {body}"
    );
    (reported, template_body)
}

/// One rendered template body, read back with this service's own template reader — the
/// same reader every operation that takes a `TemplateBody` hands a submitted body to.
fn parsed_template(case: &str, body: &str) -> template::Template {
    template::parse(body)
        .unwrap_or_else(|| panic!("case {case:?}: expected a readable template, got {body:?}"))
}

/// The resources one parsed template declares, as `(logical id, type)` pairs sorted by
/// logical id.
fn declared_resources(parsed: &template::Template) -> Vec<(String, String)> {
    let mut resources: Vec<(String, String)> = parsed
        .resources
        .iter()
        .map(|resource| (resource.logical_id.clone(), resource.resource_type.clone()))
        .collect();
    resources.sort();
    resources
}

/// The resources one rendered template body declares, in the same order.
fn declared(case: &str, body: &str) -> Vec<(String, String)> {
    declared_resources(&parsed_template(case, body))
}

/// The template a generation produced is rendered from the record: the covered resources
/// keyed by their final logical ids, carrying the record's configuration and the properties
/// it identifies each resource by, in the language the read asked for.
#[tokio::test]
async fn the_generated_template_body_is_rendered_from_the_record() {
    let router = router();

    for (index, row) in BODY_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("body-{index}");
        let stack_name = format!("body-base-stack-{index}");
        if let Some(stack) = row.template {
            deploy(&router, &stack_name, stack).await;
        }
        let members = create_members(row, row.template.map(|_| stack_name.as_str()));
        let (status, body) = create_generated_template(&router, &name, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let generated_template_id = xml_text(&body, "GeneratedTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a GeneratedTemplateId, got {body}")
        });
        wait_for_the_generation().await;

        let (status, body) = get_generated_template(&router, &name, row.format).await;
        let (reported, template_body) = answered_template(case, status, &body);
        assert_eq!(
            reported,
            generated::status::COMPLETE,
            "case {case:?}: expected the concluded generation's status, got {body}"
        );

        // The body is a template this service's own reader reads, and the resources it
        // declares are THIS row's.
        let parsed = parsed_template(case, &template_body);
        let reported_resources = declared_resources(&parsed);
        let expected_resources: Vec<(String, String)> = row
            .expected
            .iter()
            .map(|(logical_id, resource_type, _)| {
                (logical_id.to_string(), resource_type.to_string())
            })
            .collect();
        assert_eq!(
            reported_resources, expected_resources,
            "case {case:?}: expected the covered resources declared, got {template_body:?}"
        );

        // The template carries the id of the record it was generated out of.
        assert_eq!(
            parsed
                .metadata
                .get("TemplateId")
                .and_then(|id| id.as_str())
                .map(ToString::to_string),
            Some(generated_template_id.clone()),
            "case {case:?}: expected the record's own id in the template metadata, got \
             {template_body:?}"
        );

        for (logical_id, _, properties) in row.expected {
            let declaration = parsed
                .resources
                .iter()
                .find(|resource| resource.logical_id == *logical_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected {logical_id:?} declared, got {template_body:?}")
                });
            // The configuration the record carries is assigned to the template's
            // resources, in the resource attribute's own vocabulary; a record that carries
            // none assigns no such attribute.
            for (attribute, configured) in [
                ("DeletionPolicy", row.configuration.map(|(policy, _)| policy)),
                (
                    "UpdateReplacePolicy",
                    row.configuration.map(|(_, policy)| policy),
                ),
            ] {
                let expected = configured.map(|policy| attribute_policy(case, policy));
                assert_eq!(
                    declaration
                        .declaration
                        .get(attribute)
                        .and_then(|value| value.as_str()),
                    expected,
                    "case {case:?}: expected {attribute} {expected:?} on {logical_id:?}, got \
                     {template_body:?}"
                );
            }
            // The properties the record holds for the resource: the primary-identifier
            // pairs it was identified by. A resource the record holds none for declares no
            // properties at all.
            for (key, value) in *properties {
                assert_eq!(
                    declaration
                        .properties
                        .get(*key)
                        .and_then(|property| property.as_str()),
                    Some(*value),
                    "case {case:?}: expected the property {key:?} on {logical_id:?}, got \
                     {template_body:?}"
                );
            }
            if properties.is_empty() {
                assert!(
                    declaration.declaration.get("Properties").is_none(),
                    "case {case:?}: expected no properties on {logical_id:?}, got \
                     {template_body:?}"
                );
            }
        }

        // The language is the read's choice: a YAML body is YAML, not the JSON spelling of
        // the same document (JSON is itself valid YAML, so the reader alone cannot tell).
        if row.format == Some(generated::format::YAML) && !row.expected.is_empty() {
            assert!(
                !template_body.trim_start().starts_with('{'),
                "case {case:?}: expected a YAML template body, got {template_body:?}"
            );
            assert!(
                template_body.contains("Resources:"),
                "case {case:?}: expected a YAML Resources section, got {template_body:?}"
            );
        }
    }
}

/// A read before the record's first generation has concluded answers the empty template,
/// and the same read after it answers that generation's own template.
#[tokio::test]
async fn the_template_is_the_last_complete_generation_s() {
    let router = router();
    let case = "a generation still in flight";

    deploy(&router, "in-flight-base-stack", ONE_RESOURCE_STACK).await;
    let started = Instant::now();
    let (status, body) = create_generated_template(
        &router,
        "in-flight",
        &[member("StackName", "in-flight-base-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
    );

    let (status, body) = get_generated_template(&router, "in-flight", None).await;
    let (reported, template_body) = answered_template(case, status, &body);
    // Held to the clock, the way every suite here is: the emulator owes the in-flight
    // reading only until the conclusion is due, so a read that lost the race to the clock
    // is judged as a read after it instead.
    if started.elapsed() < generated::GENERATION_DELAY {
        assert_eq!(
            reported,
            generated::status::CREATE_IN_PROGRESS,
            "case {case:?}: expected the generation in flight, got {body}"
        );
        assert!(
            declared(case, &template_body).is_empty(),
            "case {case:?}: expected the empty template until a generation completes, got \
             {template_body:?}"
        );
    }

    wait_for_the_generation().await;
    let case = "the same record once its generation concluded";
    let (status, body) = get_generated_template(&router, "in-flight", None).await;
    let (reported, template_body) = answered_template(case, status, &body);
    assert_eq!(
        reported,
        generated::status::COMPLETE,
        "case {case:?}: expected the concluded generation's status, got {body}"
    );
    assert_eq!(
        declared(case, &template_body),
        vec![("Topic".to_string(), "AWS::SNS::Topic".to_string())],
        "case {case:?}: expected the generation's own template, got {template_body:?}"
    );
}

/// One record, either language: the JSON and the YAML spelling declare the same template.
#[tokio::test]
async fn both_languages_render_the_same_template() {
    let router = router();

    for (index, stack) in [ONE_RESOURCE_STACK, THREE_RESOURCE_STACK]
        .into_iter()
        .enumerate()
    {
        let case = format!("a base stack of {} resources", index * 2 + 1);
        let stack_name = format!("either-language-stack-{index}");
        let name = format!("either-language-{index}");
        deploy(&router, &stack_name, stack).await;
        let (status, body) = create_generated_template(
            &router,
            &name,
            &[
                member("StackName", &stack_name),
                member("TemplateConfiguration.DeletionPolicy", "RETAIN"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        wait_for_the_generation().await;

        let mut rendered = Vec::new();
        for format in [generated::format::JSON, generated::format::YAML] {
            let (status, body) = get_generated_template(&router, &name, Some(format)).await;
            let (_, template_body) = answered_template(&case, status, &body);
            rendered.push(declared(&case, &template_body));
        }
        assert_eq!(
            rendered[0], rendered[1],
            "case {case:?}: expected the same template in either language, got {rendered:?}"
        );
        assert!(
            !rendered[0].is_empty(),
            "case {case:?}: expected the covered resources declared, got {rendered:?}"
        );
        // And a read that names no format answers the JSON spelling, which is the language
        // a template body is written in when nothing says otherwise.
        let (status, body) = get_generated_template(&router, &name, None).await;
        let (_, defaulted) = answered_template(&case, status, &body);
        let (status, body) =
            get_generated_template(&router, &name, Some(generated::format::JSON)).await;
        let (_, json) = answered_template(&case, status, &body);
        assert_eq!(
            defaulted, json,
            "case {case:?}: expected the default language to be JSON, got {defaulted:?}"
        );
    }
}

/// Which record a read addresses, and that reading one reports it rather than changing it.
#[tokio::test]
async fn the_record_a_read_addresses() {
    let router = router();
    let other_region = "eu-west-1";

    deploy(&router, "addressed-base-stack", ONE_RESOURCE_STACK).await;
    let (status, body) = create_generated_template(
        &router,
        "addressed",
        &[member("StackName", "addressed-base-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a created record\": CreateGeneratedTemplate answered {status} with {body}"
    );
    let generated_template_id = xml_text(&body, "GeneratedTemplateId")
        .unwrap_or_else(|| panic!("expected a GeneratedTemplateId, got {body}"));
    wait_for_the_generation().await;

    // "The name or Amazon Resource Name (ARN) of the generated template": either address
    // resolves the same record, so either answers the same template.
    let case = "by name";
    let (status, body) = get_generated_template(&router, "addressed", None).await;
    let (by_name_status, by_name) = answered_template(case, status, &body);
    let case = "by the record's own ARN";
    let (status, body) = get_generated_template(&router, &generated_template_id, None).await;
    let (by_arn_status, by_arn) = answered_template(case, status, &body);
    assert_eq!(
        by_name, by_arn,
        "case {case:?}: expected either address to answer the same template"
    );
    assert_eq!(
        by_name_status, by_arn_status,
        "case {case:?}: expected either address to answer the same status"
    );

    // Reading reports: a second read answers the same body, and the record's status is
    // where it was.
    let case = "a second read in a row";
    let (status, body) = get_generated_template(&router, "addressed", None).await;
    let (again_status, again) = answered_template(case, status, &body);
    assert_eq!(
        (again_status.as_str(), again.as_str()),
        (by_name_status.as_str(), by_name.as_str()),
        "case {case:?}: expected the read to report rather than change the record"
    );
    let (status, body) = describe_generated_template(&router, "addressed").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(generated::status::COMPLETE),
        "case {case:?}: expected the record untouched by the reads, got {body}"
    );

    // An address no record of this Region holds: the modelled 404, naming what was asked
    // for.
    for (case, address) in [
        ("a name nothing created", "never-generated".to_string()),
        (
            "an ARN nothing created",
            format!(
                "arn:aws:cloudformation:{REGION}:000000000000:generatedtemplate/\
                 00000000-0000-0000-0000-000000000000"
            ),
        ),
    ] {
        let (status, body) = get_generated_template(&router, &address, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(generated::does_not_exist(&address).as_str()),
            "case {case:?}: expected the refusal to name what was asked for, got {body}"
        );
    }

    // And a record of ANOTHER Region is not addressable from here, by either address.
    for (case, address) in [
        ("another Region's name", "addressed".to_string()),
        ("another Region's ARN", generated_template_id.clone()),
    ] {
        let (status, body) =
            get_generated_template_in(&router, other_region, &address, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from {other_region}, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );
    }

    // Nothing above created a record in that other Region: the read refused, and refusing
    // persists nothing.
    let (status, _) = call_in(
        &router,
        other_region,
        "DescribeGeneratedTemplate",
        &[member("GeneratedTemplateName", "addressed")],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a refused read persists nothing\": expected no record in {other_region}"
    );
    // `call` is the Region every other case here is signed for; naming it keeps the import
    // honest for a suite that also calls `call_in`.
    let (status, _) = call(
        &router,
        "DescribeGeneratedTemplate",
        &[member("GeneratedTemplateName", "addressed")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the record's own Region\": expected the record still addressable here"
    );
}
