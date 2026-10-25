// SPDX-License-Identifier: CC0-1.0

//! Generated templates, end to end over the awsQuery wire: `CreateGeneratedTemplate` starts
//! a generation and `DescribeGeneratedTemplate` reads THAT generation back.
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **A generation is persisted and read back.** `CreateGeneratedTemplate` answers a
//!    `GeneratedTemplateId` in the ARN form the model documents, and
//!    `DescribeGeneratedTemplate` reports that record: the generation in flight right after
//!    the call and `COMPLETE` once the clock has made its conclusion due, the resources it
//!    covers (the base stack's, the ones the request listed itself, or both), the base
//!    stack's ARN when one was named, the `TemplateConfiguration` the request supplied, and a
//!    `TemplateProgress` counting the covered resources.
//!
//! 2. **The in-flight cap, and that it lifts.** At most
//!    [`generated::MAX_CONCURRENT_GENERATIONS`] generations may be pending or in progress at
//!    one time: of a burst of N calls the first `min(N, cap)` are answered 200 and every
//!    further call is refused HTTP 429 `ConcurrentResourcesLimitExceeded`, a refused call
//!    leaves NO record (`DescribeGeneratedTemplate` on its name answers 404
//!    `GeneratedTemplateNotFound`), and once the in-flight set has concluded the very same
//!    call succeeds and reaches `COMPLETE`.
//!
//! 3. **The refusals around it.** A second generation under a name already taken is refused
//!    400 `AlreadyExistsException`, a base stack this account does not hold is refused 400
//!    `ValidationError`, and a read of a name nothing created is refused 404
//!    `GeneratedTemplateNotFound`.
//!
//! Each table spans the class its rule generalises over rather than one recorded case: one
//! covered resource and several, resources from a base stack and resources the request lists
//! itself, a request with a template configuration and one without, and burst sizes under,
//! at and over the cap.

use std::time::{Duration, Instant};

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts, ACCOUNT_ID,
    REGION,
};

/// Wait past the instant a generation's conclusion becomes due.
pub(crate) async fn wait_for_the_generation() {
    tokio::time::sleep(generated::GENERATION_DELAY + Duration::from_millis(250)).await;
}

pub(crate) async fn create_generated_template(
    router: &MultiServiceRouter,
    name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("GeneratedTemplateName", name)];
    members.extend_from_slice(extra);
    call(router, "CreateGeneratedTemplate", &members).await
}

pub(crate) async fn describe_generated_template(
    router: &MultiServiceRouter,
    name: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeGeneratedTemplate",
        &[member("GeneratedTemplateName", name)],
    )
    .await
}

/// Deploy a template, wait past the instant its create settles, and answer the stack id.
pub(crate) async fn deploy(router: &MultiServiceRouter, name: &str, template: &str) -> String {
    let (status, body) = create_stack(router, name, template, &[]).await;
    assert_eq!(
        status, 200,
        "CreateStack({name}) answered {status} with {body}"
    );
    let stack_id = xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("case {name:?}: expected a stack id, got {body}"));
    wait_for_the_transition().await;
    stack_id
}

/// The `GeneratedTemplateId` of a successful `CreateGeneratedTemplate`, checked to be the
/// ARN form the model documents:
/// `arn:${Partition}:cloudformation:${Region}:${Account}:generatedtemplate/${Id}`.
pub(crate) fn assert_generated_template_id_shape(case: &str, id: &str) {
    let prefix = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:generatedtemplate/");
    assert!(
        id.starts_with(&prefix),
        "case {case:?}: expected a generated template id under {prefix:?}, got {id:?}"
    );
    let unique = &id[prefix.len()..];
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = unique.split('-').collect();
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected an 8-4-4-4-12 id after {prefix:?}, got {unique:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected {width} lowercase-hex digits, got {group:?} in {unique:?}"
        );
    }
}

/// One resource a request lists itself: `ResourceDefinition`'s type, logical id and the one
/// `ResourceIdentifier` pair that identifies it.
pub(crate) struct ListedResource {
    pub(crate) resource_type: &'static str,
    pub(crate) logical_resource_id: &'static str,
    pub(crate) identifier_key: &'static str,
    pub(crate) identifier_value: &'static str,
}

/// The request members that submit one `Resources` entry, in the awsQuery flattened form.
pub(crate) fn listed_members(position: usize, listed: &ListedResource) -> Vec<(String, String)> {
    let index = position + 1;
    vec![
        member(
            &format!("Resources.member.{index}.ResourceType"),
            listed.resource_type,
        ),
        member(
            &format!("Resources.member.{index}.LogicalResourceId"),
            listed.logical_resource_id,
        ),
        member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.1.key"),
            listed.identifier_key,
        ),
        member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.1.value"),
            listed.identifier_value,
        ),
    ]
}

/// One row of the round-trip table.
struct GenerationCase {
    case: &'static str,
    /// The base stack's template, or `None` for a request that names no base stack.
    template: Option<&'static str>,
    /// The resources the request lists itself.
    listed: &'static [ListedResource],
    /// `TemplateConfiguration`'s `DeletionPolicy` and `UpdateReplacePolicy`, when the
    /// request supplies them.
    configuration: Option<(&'static str, &'static str)>,
    /// The logical ids the generated template must report, in sorted order.
    logical_ids: &'static [&'static str],
}

/// A template declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A template declaring three resources of two types, so a row covers more than one.
const THREE_RESOURCES: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Beta":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// The rows of the round-trip table: one covered resource and several, resources from a base
/// stack and resources the request lists itself, with a template configuration and without.
const GENERATION_CASES: &[GenerationCase] = &[
    GenerationCase {
        case: "one resource, from a base stack",
        template: Some(ONE_RESOURCE),
        listed: &[],
        configuration: None,
        logical_ids: &["Topic"],
    },
    GenerationCase {
        case: "three resources of two types, from a base stack, with a configuration",
        template: Some(THREE_RESOURCES),
        listed: &[],
        configuration: Some(("RETAIN", "DELETE")),
        logical_ids: &["Alpha", "Beta", "Bucket"],
    },
    GenerationCase {
        case: "no base stack: two resources the request lists itself",
        template: None,
        listed: &[
            ListedResource {
                resource_type: "AWS::SNS::Topic",
                logical_resource_id: "ListedTopic",
                identifier_key: "TopicArn",
                identifier_value: "arn:aws:sns:us-east-1:000000000000:a-listed-topic",
            },
            ListedResource {
                resource_type: "AWS::DynamoDB::Table",
                logical_resource_id: "ListedTable",
                identifier_key: "TableName",
                identifier_value: "a-listed-table",
            },
        ],
        configuration: Some(("DELETE", "RETAIN")),
        logical_ids: &["ListedTable", "ListedTopic"],
    },
    GenerationCase {
        case: "a base stack plus a resource the request lists itself",
        template: Some(ONE_RESOURCE),
        listed: &[ListedResource {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: "ListedBucket",
            identifier_key: "BucketName",
            identifier_value: "a-listed-bucket",
        }],
        configuration: None,
        logical_ids: &["ListedBucket", "Topic"],
    },
];

/// A generation is persisted by the create and reported by the read: in flight straight
/// after the call, complete once the clock has made its conclusion due, over the resources
/// and the configuration the call named.
#[tokio::test]
async fn a_generation_is_read_back_through_describe_generated_template() {
    let router = router();

    for (index, row) in GENERATION_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("generated-template-{index}");
        let mut extra: Vec<(String, String)> = Vec::new();
        let stack_id = match row.template {
            Some(template) => {
                let stack = format!("base-stack-{index}");
                let stack_id = deploy(&router, &stack, template).await;
                extra.push(member("StackName", &stack));
                Some(stack_id)
            }
            None => None,
        };
        for (position, listed) in row.listed.iter().enumerate() {
            extra.extend(listed_members(position, listed));
        }
        if let Some((deletion_policy, update_replace_policy)) = row.configuration {
            extra.push(member(
                "TemplateConfiguration.DeletionPolicy",
                deletion_policy,
            ));
            extra.push(member(
                "TemplateConfiguration.UpdateReplacePolicy",
                update_replace_policy,
            ));
        }

        let started = Instant::now();
        let (status, body) = create_generated_template(&router, &name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let generated_template_id = xml_text(&body, "GeneratedTemplateId").unwrap_or_else(|| {
            panic!("case {case:?}: expected a GeneratedTemplateId, got {body}")
        });
        assert_generated_template_id_shape(case, &generated_template_id);

        // Read straight after the call: the generation is still in flight until its
        // conclusion is due, which is the status — and the id — a poller sees first.
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        let reported_status = xml_text(&body, "Status");
        let in_flight = started.elapsed() < generated::GENERATION_DELAY;
        if in_flight {
            assert_eq!(
                reported_status.as_deref(),
                Some(generated::status::CREATE_IN_PROGRESS),
                "case {case:?}: expected the generation in flight, got {body}"
            );
            assert_eq!(
                xml_text(&body, "ResourcesProcessing"),
                Some(row.logical_ids.len().to_string()),
                "case {case:?}: expected every covered resource in process, got {body}"
            );
        }
        assert_eq!(
            xml_text(&body, "GeneratedTemplateId").as_deref(),
            Some(generated_template_id.as_str()),
            "case {case:?}: expected the id the create answered, got {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the name the create was given, got {body}"
        );

        // And once the clock has made the conclusion due, the same read reports it.
        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected the generation complete, got {body}"
        );
        assert_eq!(
            xml_text(&body, "StatusReason").as_deref(),
            Some(generated::GENERATION_COMPLETE_REASON),
            "case {case:?}: expected the completed generation's reason, got {body}"
        );
        let mut reported: Vec<String> = xml_texts(&body, "LogicalResourceId");
        reported.sort();
        assert_eq!(
            reported, row.logical_ids,
            "case {case:?}: expected the covered resources' logical ids, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesSucceeded"),
            Some(row.logical_ids.len().to_string()),
            "case {case:?}: expected every covered resource succeeded, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesProcessing").as_deref(),
            Some("0"),
            "case {case:?}: expected nothing still in process, got {body}"
        );
        assert_eq!(
            xml_text(&body, "TotalWarnings").as_deref(),
            Some("0"),
            "case {case:?}: expected no warnings, got {body}"
        );
        assert_eq!(
            xml_text(&body, "StackId"),
            stack_id,
            "case {case:?}: expected the base stack's ARN, got {body}"
        );
        assert_eq!(
            xml_text(&body, "DeletionPolicy").as_deref(),
            row.configuration.map(|(deletion, _)| deletion),
            "case {case:?}: expected the configuration the request supplied, got {body}"
        );
        assert_eq!(
            xml_text(&body, "UpdateReplacePolicy").as_deref(),
            row.configuration.map(|(_, update_replace)| update_replace),
            "case {case:?}: expected the configuration the request supplied, got {body}"
        );
        // A resource the REQUEST identified is reported under the identifier it supplied;
        // the identifiers are the only ones reported, since a base stack's resource carries
        // no primary-identifier property this emulator records.
        let mut keys: Vec<String> = xml_texts(&body, "key");
        keys.sort();
        let mut expected_keys: Vec<String> = row
            .listed
            .iter()
            .map(|listed| listed.identifier_key.to_string())
            .collect();
        expected_keys.sort();
        assert_eq!(
            keys, expected_keys,
            "case {case:?}: expected the listed resources' identifier keys, got {body}"
        );
        for listed in row.listed {
            assert!(
                xml_texts(&body, "value").contains(&listed.identifier_value.to_string()),
                "case {case:?}: expected the identifier {:?} the request supplied, got {body}",
                listed.identifier_value
            );
        }
        let statuses = xml_texts(&body, "ResourceStatus");
        assert_eq!(
            statuses,
            vec![generated::resource_status::COMPLETE.to_string(); row.logical_ids.len()],
            "case {case:?}: expected every covered resource complete, got {body}"
        );
    }
}

/// The code of an awsQuery error response.
pub(crate) fn error_code(case: &str, body: &str) -> String {
    xml_text(body, "Code")
        .unwrap_or_else(|| panic!("case {case:?}: expected an error <Code>, got {body}"))
}

/// Of a burst of calls, the ones within the in-flight cap succeed and the rest are refused
/// 429 `ConcurrentResourcesLimitExceeded`; a refused call leaves no record behind; and once
/// the in-flight set has concluded the refused call succeeds.
#[tokio::test]
async fn the_in_flight_cap_refuses_the_excess_and_then_lifts() {
    let router = router();
    let cap = generated::MAX_CONCURRENT_GENERATIONS;
    // Burst sizes under the cap, exactly at it, one past it and well past it.
    let bursts = [1usize, cap, cap + 1, cap + 3];

    for (round, burst) in bursts.into_iter().enumerate() {
        let case = format!("a burst of {burst} calls against a cap of {cap}");
        let names: Vec<String> = (0..burst)
            .map(|index| format!("burst-{round}-{index}"))
            .collect();

        let mut accepted: Vec<String> = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        let started = Instant::now();
        for name in &names {
            let (status, body) = create_generated_template(&router, name, &[]).await;
            if status == 200 {
                assert!(
                    xml_text(&body, "GeneratedTemplateId").is_some(),
                    "case {case:?}: expected an id on the accepted call, got {body}"
                );
                accepted.push(name.clone());
            } else {
                assert_eq!(
                    status, 429,
                    "case {case:?}: expected the excess refused 429, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&case, &body),
                    "ConcurrentResourcesLimitExceeded",
                    "case {case:?}: expected the modelled concurrency refusal, got {body}"
                );
                refused.push(name.clone());
            }
        }

        // The counts are exact only while the whole burst is still in flight; on a loaded
        // machine an early generation can conclude mid-burst and free a slot, so the strict
        // row is held to the clock the way `stack_lifecycle::expected_status` is, and the
        // invariant that the cap is never exceeded is asserted either way.
        assert!(
            accepted.len() <= cap,
            "case {case:?}: expected at most {cap} accepted, accepted {accepted:?} and \
             refused {refused:?}"
        );
        assert_eq!(
            accepted.len() + refused.len(),
            burst,
            "case {case:?}: expected every call either accepted or refused, accepted \
             {accepted:?} and refused {refused:?}"
        );
        if started.elapsed() < generated::GENERATION_DELAY {
            assert_eq!(
                accepted.len(),
                burst.min(cap),
                "case {case:?}: expected {} of {burst} calls accepted, accepted {accepted:?} \
                 and refused {refused:?}",
                burst.min(cap)
            );
            assert_eq!(
                refused.len(),
                burst - burst.min(cap),
                "case {case:?}: expected {} of {burst} calls refused, accepted {accepted:?} \
                 and refused {refused:?}",
                burst - burst.min(cap)
            );
        }

        // A refused call left NOTHING behind: its name is not addressable at all.
        for name in &refused {
            let (status, body) = describe_generated_template(&router, name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused name unknown, got {status} with {body}"
            );
            assert_eq!(
                error_code(&case, &body),
                "GeneratedTemplateNotFound",
                "case {case:?}: expected the modelled not-found refusal, got {body}"
            );
        }

        // The cap counts the generations still in flight, so it lifts as they conclude: the
        // very call it refused now succeeds and reaches the terminal status.
        wait_for_the_generation().await;
        for name in &refused {
            let (status, body) = create_generated_template(&router, name, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name:?} accepted once the cap lifted, got {status} \
                 with {body}"
            );
        }
        wait_for_the_generation().await;
        for name in accepted.iter().chain(refused.iter()) {
            let (status, body) = describe_generated_template(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeGeneratedTemplate({name:?}) answered {status} with \
                 {body}"
            );
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(generated::status::COMPLETE),
                "case {case:?}: expected {name:?} complete, got {body}"
            );
        }
    }
}

/// The refusals around a generation: a name already taken, a base stack that does not
/// exist, and a read of a name nothing created.
#[tokio::test]
async fn the_refusals_around_a_generation() {
    let router = router();

    // A read of a name nothing created: the modelled 404.
    let (status, body) = describe_generated_template(&router, "never-created").await;
    assert_eq!(
        status, 404,
        "case \"a name nothing created\": expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code("a name nothing created", &body),
        "GeneratedTemplateNotFound",
        "case \"a name nothing created\": expected the modelled not-found refusal, got {body}"
    );

    // A base stack this account does not hold: refused in CloudFormation's own wording,
    // and nothing is created under the name.
    let (status, body) = create_generated_template(
        &router,
        "from-a-missing-stack",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a base stack that does not exist\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code("a base stack that does not exist", &body),
        "ValidationError",
        "case \"a base stack that does not exist\": expected the service's refusal, got {body}"
    );
    let (status, _) = describe_generated_template(&router, "from-a-missing-stack").await;
    assert_eq!(
        status, 404,
        "case \"a base stack that does not exist\": expected no record left behind"
    );

    // A name already taken, both while its generation is in flight and after it concluded.
    let (status, body) = create_generated_template(&router, "taken", &[]).await;
    assert_eq!(
        status, 200,
        "case \"a name already taken\": expected the first call accepted, got {status} with \
         {body}"
    );
    for case in ["a name already taken, in flight", "a name already taken, complete"] {
        let (status, body) = create_generated_template(&router, "taken", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "AlreadyExistsException",
            "case {case:?}: expected the modelled already-exists refusal, got {body}"
        );
        wait_for_the_generation().await;
    }
}
