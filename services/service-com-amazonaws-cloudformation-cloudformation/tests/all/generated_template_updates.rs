// SPDX-License-Identifier: CC0-1.0

//! `UpdateGeneratedTemplate`, end to end over the awsQuery wire: the record an update
//! rewrites, and the generation it starts over it.
//!
//! RULE: `UpdateGeneratedTemplate` rewrites the generated template its
//! `GeneratedTemplateName` addresses in the account and Region the request is signed for —
//! the name it is addressable by, the resources it covers (removed by logical id, added as
//! the request defines them, refreshed against the live state of the account) and the
//! `DeletionPolicy` / `UpdateReplacePolicy` its `TemplateConfiguration` assigns — leaving
//! every member the request did not name as it was, answers the record's OWN id, and starts
//! a generation that is in flight until the clock makes its conclusion due. An address no
//! record of that Region holds is refused 404 `GeneratedTemplateNotFound`, and a rename onto
//! a name another record already answers for is refused 400 `AlreadyExistsException` with
//! the record left as it was.
//!
//! Four suites, one per face of that rule, each spanning the class the rule generalises over
//! rather than one recorded call:
//!
//! 1. **What the update rewrites.** A rename alone, one resource added and two, an add that
//!    lands on a logical id the record already covers, one resource removed and two, a
//!    remove and an add in the same call (including the same logical id in both), one
//!    setting of the configuration changed while the other is left alone, and a record
//!    generated from a base stack as well as one holding only resources the request listed
//!    itself.
//! 2. **The live-state refresh.** `RefreshAllResources` over a record whose resources
//!    carried no identifier at all, one whose resource nothing live answers for, and one
//!    whose resource the request named by a live physical id under a key of its own.
//! 3. **Which generation's template a read answers.** The update's own generation is in
//!    flight, so `GetGeneratedTemplate` keeps answering the last COMPLETE generation's
//!    template until this one concludes.
//! 4. **The refusals around it.** A name nothing created, a record of another Region, a
//!    rename onto a name another record holds (and that the refused call left both records
//!    alone), a rename onto the record's own name, and an update addressed by the record's
//!    ARN.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_template_body::{declared, get_generated_template};
use crate::generated_templates::{
    assert_generated_template_id_shape, create_generated_template, deploy,
    describe_generated_template, error_code, listed_members, wait_for_the_generation,
    ListedResource,
};
use crate::stack_lifecycle::{call, call_in, member, router, xml_text, xml_texts};

/// A base stack declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring two resources of two types.
const TWO_RESOURCE_STACK: &str = r#"{"Resources":{
    "Topic":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// A base stack declaring three resources of two types, so a row can remove more than one.
const THREE_RESOURCE_STACK: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Beta":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// `UpdateGeneratedTemplate` for one address, with whatever else the case names.
async fn update_generated_template(
    router: &MultiServiceRouter,
    name_or_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("GeneratedTemplateName", name_or_id)];
    members.extend_from_slice(extra);
    call(router, "UpdateGeneratedTemplate", &members).await
}

/// The request members that submit one `AddResources` entry, in the awsQuery flattened form.
fn add_members(position: usize, listed: &ListedResource) -> Vec<(String, String)> {
    let index = position + 1;
    vec![
        member(
            &format!("AddResources.member.{index}.ResourceType"),
            listed.resource_type,
        ),
        member(
            &format!("AddResources.member.{index}.LogicalResourceId"),
            listed.logical_resource_id,
        ),
        member(
            &format!("AddResources.member.{index}.ResourceIdentifier.entry.1.key"),
            listed.identifier_key,
        ),
        member(
            &format!("AddResources.member.{index}.ResourceIdentifier.entry.1.value"),
            listed.identifier_value,
        ),
    ]
}

/// The request members that submit a `RemoveResources` list.
fn remove_members(logical_ids: &[&str]) -> Vec<(String, String)> {
    logical_ids
        .iter()
        .enumerate()
        .map(|(position, logical_id)| {
            member(
                &format!("RemoveResources.member.{}", position + 1),
                logical_id,
            )
        })
        .collect()
}

/// One row of the rewrite table.
struct UpdateCase {
    case: &'static str,
    /// The base stack's template, or `None` for a record that names no base stack.
    template: Option<&'static str>,
    /// The resources the CREATE listed itself.
    created: &'static [ListedResource],
    /// The `TemplateConfiguration` the create supplied, as `(DeletionPolicy,
    /// UpdateReplacePolicy)`.
    created_configuration: Option<(&'static str, &'static str)>,
    /// Whether the update renames the record.
    rename: bool,
    /// The resources the update adds.
    add: &'static [ListedResource],
    /// The logical ids the update removes.
    remove: &'static [&'static str],
    /// The `TemplateConfiguration` settings the update supplies, by member name, so a row
    /// can change one of the two and leave the other alone.
    configuration: &'static [(&'static str, &'static str)],
    /// The logical ids the record must cover after the update, sorted.
    logical_ids: &'static [&'static str],
    /// The `(DeletionPolicy, UpdateReplacePolicy)` the record must report after it.
    policies: (Option<&'static str>, Option<&'static str>),
}

/// The rows: every way the rule says an update rewrites a record, over records of both
/// origins.
const UPDATE_CASES: &[UpdateCase] = &[
    UpdateCase {
        case: "a rename alone leaves the covered resources and the policies where they were",
        template: Some(ONE_RESOURCE_STACK),
        created: &[],
        created_configuration: Some(("RETAIN", "DELETE")),
        rename: true,
        add: &[],
        remove: &[],
        configuration: &[],
        logical_ids: &["Topic"],
        policies: (Some("RETAIN"), Some("DELETE")),
    },
    UpdateCase {
        case: "one resource added to the ones a base stack contributed",
        template: Some(ONE_RESOURCE_STACK),
        created: &[],
        created_configuration: None,
        rename: false,
        add: &[ListedResource {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: "AddedBucket",
            identifier_key: "BucketName",
            identifier_value: "an-added-bucket",
        }],
        remove: &[],
        configuration: &[],
        logical_ids: &["AddedBucket", "Topic"],
        policies: (None, None),
    },
    UpdateCase {
        case: "two resources added, one of them onto a logical id the record already covers",
        template: Some(THREE_RESOURCE_STACK),
        created: &[],
        created_configuration: None,
        rename: false,
        add: &[
            ListedResource {
                resource_type: "AWS::SNS::Topic",
                logical_resource_id: "Alpha",
                identifier_key: "TopicArn",
                identifier_value: "arn:aws:sns:us-east-1:000000000000:an-identified-alpha",
            },
            ListedResource {
                resource_type: "AWS::DynamoDB::Table",
                logical_resource_id: "AddedTable",
                identifier_key: "TableName",
                identifier_value: "an-added-table",
            },
        ],
        remove: &[],
        configuration: &[],
        logical_ids: &["AddedTable", "Alpha", "Beta", "Bucket"],
        policies: (None, None),
    },
    UpdateCase {
        case: "one resource removed by its logical id",
        template: Some(THREE_RESOURCE_STACK),
        created: &[],
        created_configuration: None,
        rename: false,
        add: &[],
        remove: &["Beta"],
        configuration: &[],
        logical_ids: &["Alpha", "Bucket"],
        policies: (None, None),
    },
    UpdateCase {
        case: "two resources removed and one added in the same call",
        template: Some(THREE_RESOURCE_STACK),
        created: &[],
        created_configuration: None,
        rename: true,
        add: &[ListedResource {
            resource_type: "AWS::DynamoDB::Table",
            logical_resource_id: "AddedTable",
            identifier_key: "TableName",
            identifier_value: "an-added-table",
        }],
        remove: &["Alpha", "Beta"],
        configuration: &[],
        logical_ids: &["AddedTable", "Bucket"],
        policies: (None, None),
    },
    UpdateCase {
        case: "a resource removed and re-added under the same logical id in one call",
        template: Some(THREE_RESOURCE_STACK),
        created: &[],
        created_configuration: None,
        rename: false,
        add: &[ListedResource {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: "Bucket",
            identifier_key: "BucketName",
            identifier_value: "a-re-added-bucket",
        }],
        remove: &["Bucket"],
        configuration: &[],
        logical_ids: &["Alpha", "Beta", "Bucket"],
        policies: (None, None),
    },
    UpdateCase {
        case: "one setting of the configuration changed, the other left alone",
        template: Some(ONE_RESOURCE_STACK),
        created: &[],
        created_configuration: Some(("RETAIN", "RETAIN")),
        rename: false,
        add: &[],
        remove: &[],
        configuration: &[("DeletionPolicy", "DELETE")],
        logical_ids: &["Topic"],
        policies: (Some("DELETE"), Some("RETAIN")),
    },
    UpdateCase {
        case: "no base stack: a configuration assigned to resources the create listed itself",
        template: None,
        created: &[ListedResource {
            resource_type: "AWS::SNS::Topic",
            logical_resource_id: "ListedTopic",
            identifier_key: "TopicArn",
            identifier_value: "arn:aws:sns:us-east-1:000000000000:a-listed-topic",
        }],
        created_configuration: None,
        rename: true,
        add: &[],
        remove: &[],
        configuration: &[
            ("DeletionPolicy", "RETAIN"),
            ("UpdateReplacePolicy", "RETAIN"),
        ],
        logical_ids: &["ListedTopic"],
        policies: (Some("RETAIN"), Some("RETAIN")),
    },
];

/// An update rewrites exactly what it names on the record it addresses, answers the record's
/// own id, and starts a generation that concludes on the clock.
#[tokio::test]
async fn an_update_rewrites_the_record_and_starts_a_new_generation() {
    let router = router();

    for (index, row) in UPDATE_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("updated-template-{index}");

        let mut created: Vec<(String, String)> = Vec::new();
        if let Some(template) = row.template {
            let stack = format!("update-base-stack-{index}");
            deploy(&router, &stack, template).await;
            created.push(member("StackName", &stack));
        }
        for (position, listed) in row.created.iter().enumerate() {
            created.extend(listed_members(position, listed));
        }
        if let Some((deletion_policy, update_replace_policy)) = row.created_configuration {
            created.push(member(
                "TemplateConfiguration.DeletionPolicy",
                deletion_policy,
            ));
            created.push(member(
                "TemplateConfiguration.UpdateReplacePolicy",
                update_replace_policy,
            ));
        }
        let (status, body) = create_generated_template(&router, &name, &created).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let generated_template_id = xml_text(&body, "GeneratedTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a GeneratedTemplateId, got {body}")
        });
        assert_generated_template_id_shape(case, &generated_template_id);
        wait_for_the_generation().await;

        let new_name = format!("{name}-renamed");
        let mut members: Vec<(String, String)> = Vec::new();
        if row.rename {
            members.push(member("NewGeneratedTemplateName", &new_name));
        }
        for (position, listed) in row.add.iter().enumerate() {
            members.extend(add_members(position, listed));
        }
        members.extend(remove_members(row.remove));
        for (setting, value) in row.configuration {
            members.push(member(&format!("TemplateConfiguration.{setting}"), value));
        }

        let started = Instant::now();
        let (status, body) = update_generated_template(&router, &name, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateGeneratedTemplate answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateId").as_deref(),
            Some(generated_template_id.as_str()),
            "case {case:?}: expected the record's own id, which the update does not change, \
             got {body}"
        );

        // The record is addressed by the name it now carries, and the name it carried before
        // a rename addresses nothing.
        let addressed = if row.rename { new_name.clone() } else { name.clone() };
        let (status, body) = describe_generated_template(&router, &addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        // Read straight after the call the update's generation is still in flight, which is
        // the status a poller sees first. Held to the clock: on a loaded machine the
        // conclusion can already be due by the time the read lands.
        if started.elapsed() < generated::GENERATION_DELAY {
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(generated::status::UPDATE_IN_PROGRESS),
                "case {case:?}: expected the update's generation in flight, got {body}"
            );
        }
        if row.rename {
            let (status, body) = describe_generated_template(&router, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the name before the rename to address nothing, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_code(case, &body),
                "GeneratedTemplateNotFound",
                "case {case:?}: expected the modelled not-found refusal, got {body}"
            );
        }

        // And once the clock has made the update's conclusion due, the record reports it.
        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected the update's generation complete, got {body}"
        );
        assert_eq!(
            xml_text(&body, "StatusReason").as_deref(),
            Some(generated::GENERATION_COMPLETE_REASON),
            "case {case:?}: expected the completed generation's reason, got {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateName").as_deref(),
            Some(addressed.as_str()),
            "case {case:?}: expected the name the record now carries, got {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateId").as_deref(),
            Some(generated_template_id.as_str()),
            "case {case:?}: expected the record's own id, got {body}"
        );
        let mut reported: Vec<String> = xml_texts(&body, "LogicalResourceId");
        reported.sort();
        assert_eq!(
            reported, row.logical_ids,
            "case {case:?}: expected the resources the update left covered, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesSucceeded"),
            Some(row.logical_ids.len().to_string()),
            "case {case:?}: expected every covered resource succeeded, got {body}"
        );
        assert_eq!(
            xml_text(&body, "DeletionPolicy").as_deref(),
            row.policies.0,
            "case {case:?}: expected the deletion policy the record now carries, got {body}"
        );
        assert_eq!(
            xml_text(&body, "UpdateReplacePolicy").as_deref(),
            row.policies.1,
            "case {case:?}: expected the update-replace policy the record now carries, got \
             {body}"
        );
        let values = xml_texts(&body, "value");
        for listed in row.add {
            assert!(
                values.contains(&listed.identifier_value.to_string()),
                "case {case:?}: expected the identifier {:?} the update supplied, got {body}",
                listed.identifier_value
            );
        }

        // The TEMPLATE the record renders is the updated one too: a resource the update
        // removed is declared nowhere in it, and one it added is.
        let (status, body) = get_generated_template(&router, &addressed, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetGeneratedTemplate answered {status} with {body}"
        );
        let template_body = xml_text(&body, "TemplateBody")
            .unwrap_or_else(|| panic!("case {case:?}: expected a TemplateBody, got {body}"));
        let keys: Vec<String> = declared(case, &template_body)
            .into_iter()
            .map(|(logical_id, _)| logical_id)
            .collect();
        assert_eq!(
            keys, row.logical_ids,
            "case {case:?}: expected the updated template to declare the covered resources, \
             got {template_body}"
        );
    }
}

/// What the refresh must leave on a record's resources.
enum Refreshed {
    /// Every resource carries the identifier the live resource it stands for is identified
    /// by: a record whose resources carried none of their own.
    LiveIdentifiers,
    /// The identifier the request supplied, untouched: nothing live answers for the
    /// resource, so there is no live state to report for it.
    Unchanged,
    /// The same identifier VALUE, under the key the live state identifies the resource by
    /// rather than the one the request chose.
    ReKeyed,
}

/// One row of the refresh table.
struct RefreshCase {
    case: &'static str,
    /// A stack to deploy, whose live resources are the state a refresh reads.
    template: Option<&'static str>,
    /// Whether the create names that stack as the record's base stack.
    base: bool,
    /// A resource the create lists itself: `(type, logical id, identifier key, identifier
    /// value)`, where a `None` value means the physical id of the deployed stack's own
    /// resource — a value only the live state knows.
    listed: Option<(
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
    )>,
    expected: Refreshed,
}

const REFRESH_CASES: &[RefreshCase] = &[
    RefreshCase {
        case: "a record generated from a base stack, whose resources carried no identifier",
        template: Some(TWO_RESOURCE_STACK),
        base: true,
        listed: None,
        expected: Refreshed::LiveIdentifiers,
    },
    RefreshCase {
        case: "a listed resource nothing live answers for",
        template: None,
        base: false,
        listed: Some((
            "AWS::DynamoDB::Table",
            "ListedTable",
            "TableName",
            Some("not-a-live-table"),
        )),
        expected: Refreshed::Unchanged,
    },
    RefreshCase {
        case: "a listed resource named by a live physical id under a key of the request's own",
        template: Some(ONE_RESOURCE_STACK),
        base: false,
        listed: Some(("AWS::SNS::Topic", "LiveTopic", "Id", None)),
        expected: Refreshed::ReKeyed,
    },
];

/// The physical ids of one stack's resources, as `DescribeStackResources` reports them: the
/// live state a refresh is checked against, read through the service's own wire rather than
/// written down here.
async fn stack_physical_ids(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack:?}) answered {status} with {body}"
    );
    let mut ids = xml_texts(&body, "PhysicalResourceId");
    ids.sort();
    ids
}

/// `RefreshAllResources` reports the live state of the resources a generated template
/// covers: the identifier each one is really identified by right now.
#[tokio::test]
async fn a_refresh_reports_the_live_state_of_the_covered_resources() {
    let router = router();

    for (index, row) in REFRESH_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("refreshed-template-{index}");

        let mut live_ids: Vec<String> = Vec::new();
        let mut created: Vec<(String, String)> = Vec::new();
        if let Some(template) = row.template {
            let stack = format!("refresh-base-stack-{index}");
            deploy(&router, &stack, template).await;
            live_ids = stack_physical_ids(&router, &stack).await;
            assert!(
                !live_ids.is_empty(),
                "case {case:?}: expected the deployed stack's resources to carry physical \
                 ids, got {live_ids:?}"
            );
            if row.base {
                created.push(member("StackName", &stack));
            }
        }
        if let Some((resource_type, logical_id, key, value)) = row.listed {
            let value = match value {
                Some(value) => value.to_string(),
                None => live_ids
                    .first()
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a live physical id")),
            };
            created.extend([
                member("Resources.member.1.ResourceType", resource_type),
                member("Resources.member.1.LogicalResourceId", logical_id),
                member("Resources.member.1.ResourceIdentifier.entry.1.key", key),
                member("Resources.member.1.ResourceIdentifier.entry.1.value", &value),
            ]);
        }

        let (status, body) = create_generated_template(&router, &name, &created).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        let before: Vec<String> = xml_texts(&body, "key");

        let (status, body) = update_generated_template(
            &router,
            &name,
            &[member("RefreshAllResources", "true")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateGeneratedTemplate answered {status} with {body}"
        );
        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        let keys: Vec<String> = xml_texts(&body, "key");
        let mut values: Vec<String> = xml_texts(&body, "value");
        values.sort();

        match row.expected {
            Refreshed::LiveIdentifiers => {
                assert!(
                    before.is_empty(),
                    "case {case:?}: expected the base stack's resources to carry no \
                     identifier before the refresh, got {before:?}"
                );
                assert_eq!(
                    values, live_ids,
                    "case {case:?}: expected the live physical ids of the covered resources, \
                     got {body}"
                );
                assert_eq!(
                    keys.len(),
                    live_ids.len(),
                    "case {case:?}: expected one identifier per covered resource, got {keys:?}"
                );
                assert!(
                    keys.iter().all(|key| !key.is_empty()),
                    "case {case:?}: expected every identifier keyed by a property, got \
                     {keys:?}"
                );
            }
            Refreshed::Unchanged => {
                let (_, _, key, value) =
                    row.listed.expect("this row lists a resource of its own");
                let value = value.expect("this row's identifier is the request's own");
                assert_eq!(
                    keys,
                    vec![key.to_string()],
                    "case {case:?}: expected the key the request supplied, got {body}"
                );
                assert_eq!(
                    values,
                    vec![value.to_string()],
                    "case {case:?}: expected the identifier the request supplied, got {body}"
                );
            }
            Refreshed::ReKeyed => {
                let (_, _, key, _) = row.listed.expect("this row lists a resource of its own");
                assert_eq!(
                    values, live_ids,
                    "case {case:?}: expected the live physical id of the covered resource, \
                     got {body}"
                );
                assert_eq!(
                    keys.len(),
                    1,
                    "case {case:?}: expected one identifier for the covered resource, got \
                     {keys:?}"
                );
                assert!(
                    keys.iter().all(|reported| reported != key && !reported.is_empty()),
                    "case {case:?}: expected the property the live state identifies the \
                     resource by rather than the request's {key:?}, got {keys:?}"
                );
            }
        }
    }
}

/// While the update's own generation is in flight the record's body is still the last
/// COMPLETE generation's; once that generation concludes it is the updated one.
#[tokio::test]
async fn the_body_follows_the_update_only_once_its_generation_concludes() {
    let router = router();
    let case = "a resource added while the previous generation's template is readable";

    deploy(&router, "body-base-stack", ONE_RESOURCE_STACK).await;
    let (status, body) = create_generated_template(
        &router,
        "body-updated-template",
        &[member("StackName", "body-base-stack")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
    );
    wait_for_the_generation().await;

    let before = read_declared(&router, case, "body-updated-template").await;
    assert_eq!(
        before,
        vec![("Topic".to_string(), "AWS::SNS::Topic".to_string())],
        "case {case:?}: expected the base stack's resource, got {before:?}"
    );

    let started = Instant::now();
    let (status, body) = update_generated_template(
        &router,
        "body-updated-template",
        &add_members(
            0,
            &ListedResource {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: "AddedBucket",
                identifier_key: "BucketName",
                identifier_value: "a-bucket-added-by-the-update",
            },
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateGeneratedTemplate answered {status} with {body}"
    );

    let during = read_declared(&router, case, "body-updated-template").await;
    if started.elapsed() < generated::GENERATION_DELAY {
        assert_eq!(
            during, before,
            "case {case:?}: expected the last complete generation's template while the \
             update is in flight, got {during:?}"
        );
    }

    wait_for_the_generation().await;
    let after = read_declared(&router, case, "body-updated-template").await;
    assert_eq!(
        after,
        vec![
            ("AddedBucket".to_string(), "AWS::S3::Bucket".to_string()),
            ("Topic".to_string(), "AWS::SNS::Topic".to_string()),
        ],
        "case {case:?}: expected the updated template once the generation concluded, got \
         {after:?}"
    );
}

/// The resources the template one record renders declares, as `(logical id, type)` pairs.
async fn read_declared(
    router: &MultiServiceRouter,
    case: &str,
    name_or_id: &str,
) -> Vec<(String, String)> {
    let (status, body) = get_generated_template(router, name_or_id, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetGeneratedTemplate answered {status} with {body}"
    );
    let template_body = xml_text(&body, "TemplateBody")
        .unwrap_or_else(|| panic!("case {case:?}: expected a TemplateBody, got {body}"));
    declared(case, &template_body)
}

/// The refusals around an update: an address no record of the Region holds, and a rename
/// onto a name another record already answers for.
#[tokio::test]
async fn the_refusals_around_an_update() {
    let router = router();

    // A name nothing created: the modelled 404.
    let case = "a name nothing created";
    let (status, body) = update_generated_template(&router, "never-created", &[]).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "GeneratedTemplateNotFound",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );

    // A record of ANOTHER Region: a name addresses one generated template of the Region the
    // request is signed for, so this one is not addressable here and is left untouched.
    let case = "a record of another Region";
    let elsewhere = "us-west-2";
    let (status, body) = call_in(
        &router,
        elsewhere,
        "CreateGeneratedTemplate",
        &[member("GeneratedTemplateName", "elsewhere")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
    );
    let (status, body) = update_generated_template(
        &router,
        "elsewhere",
        &[member("NewGeneratedTemplateName", "renamed-from-elsewhere")],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} with {body}"
    );
    wait_for_the_generation().await;
    let (status, body) = call_in(
        &router,
        elsewhere,
        "DescribeGeneratedTemplate",
        &[member("GeneratedTemplateName", "elsewhere")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the record of {elsewhere:?} untouched, got {status} with \
         {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(generated::status::COMPLETE),
        "case {case:?}: expected the record of {elsewhere:?} left on its own generation, got \
         {body}"
    );

    // Two records of this Region, so a rename can collide with one of them.
    for name in ["held", "renaming"] {
        let (status, body) = create_generated_template(&router, name, &[]).await;
        assert_eq!(
            status, 200,
            "CreateGeneratedTemplate({name:?}) answered {status} with {body}"
        );
    }
    wait_for_the_generation().await;

    let case = "a rename onto a name another record holds";
    let (status, body) = update_generated_template(
        &router,
        "renaming",
        &[member("NewGeneratedTemplateName", "held")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "AlreadyExistsException",
        "case {case:?}: expected the modelled already-exists refusal, got {body}"
    );
    // The refused call left BOTH records as they were: each still answers for its own name,
    // and neither was moved onto a new generation.
    for name in ["held", "renaming"] {
        let (status, body) = describe_generated_template(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {name:?} still addressable, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateName").as_deref(),
            Some(name),
            "case {case:?}: expected {name:?} to keep its name, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected {name:?} left on its own generation, got {body}"
        );
    }

    // A rename onto the name the record ALREADY carries is no collision: the name it would
    // take is its own.
    let case = "a rename onto the record's own name";
    let (status, body) = update_generated_template(
        &router,
        "renaming",
        &[member("NewGeneratedTemplateName", "renaming")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update accepted, got {status} with {body}"
    );
    let generated_template_id = xml_text(&body, "GeneratedTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
    wait_for_the_generation().await;

    // "The name or Amazon Resource Name (ARN) of a generated template": the record's own ARN
    // addresses it just as its name does.
    let case = "an update addressed by the record's ARN";
    let (status, body) = update_generated_template(
        &router,
        &generated_template_id,
        &[member("NewGeneratedTemplateName", "renamed-by-arn")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update accepted, got {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "GeneratedTemplateId").as_deref(),
        Some(generated_template_id.as_str()),
        "case {case:?}: expected the record's own id, got {body}"
    );
    wait_for_the_generation().await;
    let (status, body) = describe_generated_template(&router, "renamed-by-arn").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the renamed record addressable, got {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "GeneratedTemplateId").as_deref(),
        Some(generated_template_id.as_str()),
        "case {case:?}: expected the same record under its new name, got {body}"
    );
}
