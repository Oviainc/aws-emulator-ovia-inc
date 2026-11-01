// SPDX-License-Identifier: CC0-1.0

//! `DeleteGeneratedTemplate`, end to end over the awsQuery wire.
//!
//! RULE: `DeleteGeneratedTemplate` removes the generated-template record its
//! `GeneratedTemplateName` addresses — "the name or Amazon Resource Name (ARN) of a
//! generated template" — within the account and Region the request is signed for, so after
//! it nothing addresses that generated template (`DescribeGeneratedTemplate`,
//! `GetGeneratedTemplate` and `ListGeneratedTemplates` all stop answering for it) while
//! every other record is left exactly as it was, and the name it held is free to be created
//! under again. The record goes whatever status its generation is in, so a delete of what
//! the caller has just created is accepted too. An address no record of that Region holds is
//! refused 404 `GeneratedTemplateNotFound`.
//!
//! Three suites, one per face of that rule, each a table whose rows differ along what the
//! rule generalises over rather than along one recorded case:
//!
//! 1. **The record is removed.** Over generations covering no resource at all, one, two and
//!    four, taken from a base stack, listed by the request itself and both; addressed by
//!    name and by the record's own ARN; under names spanning `GeneratedTemplateName`'s own
//!    `@length(min: 1, max: 128)`. Each row is deleted out of a store holding several
//!    records, so what the deletion leaves behind is asserted too.
//! 2. **The Region the request is signed for.** The same name in two Regions is two
//!    generated templates: a delete signed for one removes only that Region's record, and
//!    neither the other Region's ARN nor — once its own record is gone — its name is
//!    deletable from here.
//! 3. **The refusal, and what is not one.** A name nothing created is refused; a record
//!    whose generation is still in flight is deleted like any other.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    create_generated_template, deploy, describe_generated_template, error_code, listed_members,
    wait_for_the_generation, ListedResource,
};
use crate::stack_lifecycle::{call, call_in, member, router, xml_text, xml_texts, REGION};

/// A base stack declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring three resources of two types.
const THREE_RESOURCE_STACK: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Beta":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// `DeleteGeneratedTemplate` for one address.
async fn delete_generated_template(
    router: &MultiServiceRouter,
    name_or_id: &str,
) -> (u16, String) {
    delete_generated_template_in(router, REGION, name_or_id).await
}

/// [`delete_generated_template`], signed for `region`: a generated template belongs to the
/// Region the call that created it was signed for, so the credential scope of the delete is
/// what decides which Region's records it can address.
async fn delete_generated_template_in(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DeleteGeneratedTemplate",
        &[member("GeneratedTemplateName", name_or_id)],
    )
    .await
}

/// `GetGeneratedTemplate` for one address, in the language the service answers by default.
async fn get_generated_template(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    call(
        router,
        "GetGeneratedTemplate",
        &[member("GeneratedTemplateName", name_or_id)],
    )
    .await
}

/// `DescribeGeneratedTemplate` signed for `region`.
async fn describe_generated_template_in(
    router: &MultiServiceRouter,
    region: &str,
    name_or_id: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeGeneratedTemplate",
        &[member("GeneratedTemplateName", name_or_id)],
    )
    .await
}

/// The names `ListGeneratedTemplates` reports for the Region the request is signed for,
/// sorted.
async fn listed_names(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let (status, body) = call_in(router, region, "ListGeneratedTemplates", &[]).await;
    assert_eq!(
        status, 200,
        "ListGeneratedTemplates in {region} answered {status} with {body}"
    );
    let mut names = xml_texts(&body, "GeneratedTemplateName");
    names.sort();
    names
}

/// Assert that `name_or_id` addresses no generated template at all: every read of the
/// feature refuses it with the modelled 404.
async fn assert_not_addressable(router: &MultiServiceRouter, case: &str, name_or_id: &str) {
    let (status, body) = describe_generated_template(router, name_or_id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected DescribeGeneratedTemplate({name_or_id:?}) refused, got \
         {status} with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "GeneratedTemplateNotFound",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );
    let (status, body) = get_generated_template(router, name_or_id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected GetGeneratedTemplate({name_or_id:?}) refused, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "GeneratedTemplateNotFound",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );
}

/// One row of the deletion table.
struct DeletionCase {
    case: &'static str,
    /// The name the generation is created under. The shapes span the member's own
    /// constraint, `GeneratedTemplateName` `@length(min: 1, max: 128)`.
    name: String,
    /// The base stack's template, or `None` for a generation given no base stack.
    template: Option<&'static str>,
    /// The resources the request lists itself.
    listed: Vec<ListedResource>,
    /// Whether the delete addresses the record by its own ARN rather than by its name.
    by_arn: bool,
}

/// The rows: generations covering nothing at all, one resource, two and four; from a base
/// stack, from the request's own list and both; deleted by name and by ARN; under the
/// shortest and the longest name the member allows.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            // `@length(min: 1)`: the shortest name the member allows, on the generation
            // that covers nothing at all.
            case: "a one-character name, no covered resource, deleted by name",
            name: "d".to_string(),
            template: None,
            listed: Vec::new(),
            by_arn: false,
        },
        DeletionCase {
            // `@length(max: 128)`: the longest name the member allows.
            case: "a 128-character name, one resource from a base stack, deleted by ARN",
            name: format!("deleted-template-{}", "y".repeat(128 - "deleted-template-".len())),
            template: Some(ONE_RESOURCE_STACK),
            listed: Vec::new(),
            by_arn: true,
        },
        DeletionCase {
            case: "a mixed-case dotted name, two resources the request lists, deleted by name",
            name: "Delete.Me_2-Generated".to_string(),
            template: None,
            listed: vec![
                ListedResource {
                    resource_type: "AWS::SNS::Topic",
                    logical_resource_id: "DeletedTopic",
                    identifier_key: "TopicArn",
                    identifier_value: "arn:aws:sns:us-east-1:000000000000:a-deleted-topic",
                },
                ListedResource {
                    resource_type: "AWS::DynamoDB::Table",
                    logical_resource_id: "DeletedTable",
                    identifier_key: "TableName",
                    identifier_value: "a-deleted-table",
                },
            ],
            by_arn: false,
        },
        DeletionCase {
            case: "three resources from a base stack plus one listed, deleted by ARN",
            name: "four-covered-resources".to_string(),
            template: Some(THREE_RESOURCE_STACK),
            listed: vec![ListedResource {
                resource_type: "AWS::S3::Bucket",
                logical_resource_id: "DeletedBucket",
                identifier_key: "BucketName",
                identifier_value: "a-deleted-bucket",
            }],
            by_arn: true,
        },
    ]
}

/// The request members that create one row's generated template: its base stack, when the
/// row has one, and the resources the row lists itself.
fn creation_members(row: &DeletionCase, stack_name: Option<&str>) -> Vec<(String, String)> {
    let mut extra: Vec<(String, String)> = Vec::new();
    if let Some(stack_name) = stack_name {
        extra.push(member("StackName", stack_name));
    }
    for (position, listed) in row.listed.iter().enumerate() {
        extra.extend(listed_members(position, listed));
    }
    extra
}

/// Deleting a generated template removes the record it addresses — by name or by its own
/// ARN — leaves every other record untouched, and frees the name to be created under again.
#[tokio::test]
async fn deleting_a_generated_template_removes_the_record_it_addresses() {
    let router = router();
    let rows = deletion_cases();

    // One record per row, created up front so each deletion happens out of a store holding
    // several records and what it LEAVES is asserted as well as what it removes. The table
    // is sized within the in-flight quota, so every generation is accepted.
    let mut ids: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let stack_name = match row.template {
            Some(template) => {
                let stack = format!("deletion-base-stack-{index}");
                deploy(&router, &stack, template).await;
                Some(stack)
            }
            None => None,
        };
        let extra = creation_members(row, stack_name.as_deref());
        let (status, body) = create_generated_template(&router, &row.name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
        ids.push(id);
    }
    // The generations are left to conclude before the deletions, so each row is deleted out
    // of a settled store and what the delete leaves behind is read back at a terminal
    // status; a record still generating is deletable too, which suite 3 is about.
    wait_for_the_generation().await;

    let mut remaining: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    remaining.sort();
    assert_eq!(
        listed_names(&router, REGION).await,
        remaining,
        "expected every created record listed before the deletions"
    );

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = &ids[index];
        let address = if row.by_arn {
            id.clone()
        } else {
            row.name.clone()
        };

        let (status, body) = delete_generated_template(&router, &address).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteGeneratedTemplate({address:?}) answered {status} with {body}"
        );
        assert!(
            xml_text(&body, "Code").is_none(),
            "case {case:?}: expected a successful response, got {body}"
        );

        // Nothing addresses the deleted generated template any more: not its name, not its
        // ARN, through no read of the feature.
        assert_not_addressable(&router, case, &row.name).await;
        assert_not_addressable(&router, case, id).await;

        // ...and a second delete of the same address is refused, since the record is gone.
        let (status, body) = delete_generated_template(&router, &address).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a second delete of {address:?} refused, got {status} with \
             {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );

        // Every record the delete did not address is still there, in full.
        remaining.retain(|name| name != &row.name);
        assert_eq!(
            listed_names(&router, REGION).await,
            remaining,
            "case {case:?}: expected only {:?} removed from the listing",
            row.name
        );
        for name in &remaining {
            let (status, body) = describe_generated_template(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name:?} untouched by the delete, got {status} with \
                 {body}"
            );
        }

        // The name the deleted record held is free: a create under it is accepted and
        // mints a NEW record rather than reviving the deleted one.
        let (status, body) = create_generated_template(&router, &row.name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {:?} free to create again, got {status} with {body}",
            row.name
        );
        let new_id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
        assert_ne!(
            &new_id, id,
            "case {case:?}: expected a new record for the re-created name, got the deleted \
             record's id {id:?}"
        );
        wait_for_the_generation().await;
        let (status, body) = delete_generated_template(&router, &new_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-created record deletable, got {status} with {body}"
        );
        assert_not_addressable(&router, case, &row.name).await;
    }

    assert!(
        listed_names(&router, REGION).await.is_empty(),
        "expected the Region to hold no generated template once every record was deleted"
    );
}

/// A delete addresses only the records of the Region it is signed for: the same name in a
/// second Region is a second generated template, and the other Region's ARN is not
/// deletable from here.
#[tokio::test]
async fn a_generated_template_is_deletable_only_in_the_region_it_was_created_in() {
    let router = router();
    // Pairs of Regions, one of which is the suite's default and one in which neither is, so
    // nothing about the rule hangs on a privileged Region.
    let pairs = [(REGION, "eu-west-2"), ("ap-northeast-1", "sa-east-1")];
    let name = "deletable-in-one-region-only";

    for (first, second) in pairs {
        let case = format!("{name:?} created in both {first} and {second}");
        let mut ids = Vec::new();
        for region in [first, second] {
            let (status, body) = call_in(
                &router,
                region,
                "CreateGeneratedTemplate",
                &[member("GeneratedTemplateName", name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateGeneratedTemplate in {region} answered {status} with {body}"
            );
            ids.push(xml_text(&body, "GeneratedTemplateId").unwrap_or_else(|| {
                panic!("case {case:?}: expected a GeneratedTemplateId in {region}, got {body}")
            }));
        }
        assert_ne!(
            ids[0], ids[1],
            "case {case:?}: expected two records, got one id {:?}",
            ids[0]
        );
        wait_for_the_generation().await;

        // The other Region's ARN addresses nothing here, so it cannot be deleted from here.
        let (status, body) = delete_generated_template_in(&router, first, &ids[1]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the {second} record undeletable from {first}, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );

        // The delete signed for the first Region removes THAT Region's record, and the
        // second Region's record of the same name is untouched.
        let (status, body) = delete_generated_template_in(&router, first, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteGeneratedTemplate in {first} answered {status} with {body}"
        );
        let (status, body) = describe_generated_template_in(&router, first, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the {first} record gone, got {status} with {body}"
        );
        let (status, body) = describe_generated_template_in(&router, second, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the {second} record untouched, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateId").as_deref(),
            Some(ids[1].as_str()),
            "case {case:?}: expected {second} to still report its own record, got {body}"
        );
        assert_eq!(
            listed_names(&router, second).await,
            vec![name.to_string()],
            "case {case:?}: expected {second} to still list its record"
        );
        assert!(
            listed_names(&router, first).await.is_empty(),
            "case {case:?}: expected {first} to list nothing once its record was deleted"
        );

        // And the second Region's own delete removes its record, leaving nothing anywhere.
        let (status, body) = delete_generated_template_in(&router, second, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteGeneratedTemplate in {second} answered {status} with {body}"
        );
        for region in [first, second] {
            assert!(
                listed_names(&router, region).await.is_empty(),
                "case {case:?}: expected {region} to list nothing once both records were \
                 deleted"
            );
        }
    }
}

/// The refusal, and what is not one: an address nothing created is refused, while a record
/// whose generation is still in flight is deleted like any other.
#[tokio::test]
async fn the_refusals_around_deleting_a_generated_template() {
    let router = router();

    // Addresses nothing created: a bare name, and an ARN in the shape this service mints.
    for address in [
        "never-created-at-all",
        "arn:aws:cloudformation:us-east-1:000000000000:generatedtemplate/\
         2e8465c1-9a80-43ea-a3a3-4f2d692fe6dc",
    ] {
        let case = format!("deleting {address:?}, which nothing created");
        let (status, body) = delete_generated_template(&router, address).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );
    }

    // A generation still in flight is discarded with its record: the delete is accepted,
    // and nothing addresses the generated template afterwards. Rows spanning the two
    // addresses a delete may use, and both covering resources and covering none, so what is
    // deleted is not about what the generation covers.
    let rows: Vec<(&str, bool, Vec<ListedResource>)> = vec![
        ("in flight, no covered resource, addressed by name", false, Vec::new()),
        (
            "in flight, one covered resource, addressed by ARN",
            true,
            vec![ListedResource {
                resource_type: "AWS::Logs::LogGroup",
                logical_resource_id: "InFlightGroup",
                identifier_key: "LogGroupName",
                identifier_value: "an-in-flight-group",
            }],
        ),
    ];

    for (index, (case, by_arn, listed)) in rows.iter().enumerate() {
        let name = format!("in-flight-{index}");
        let mut extra: Vec<(String, String)> = Vec::new();
        for (position, resource) in listed.iter().enumerate() {
            extra.extend(listed_members(position, resource));
        }
        let started = Instant::now();
        let (status, body) = create_generated_template(&router, &name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
        let address = if *by_arn { id.clone() } else { name.clone() };

        // The generation really is still in flight when the delete is sent: the record
        // reports a non-terminal status right up to the instant the clock concludes it.
        let (describe_status, described) = describe_generated_template(&router, &name).await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {describe_status} with \
             {described}"
        );
        if started.elapsed() < generated::GENERATION_DELAY {
            assert_eq!(
                xml_text(&described, "Status").as_deref(),
                Some(generated::status::CREATE_IN_PROGRESS),
                "case {case:?}: expected the generation still in flight, got {described}"
            );
        }

        let (status, body) = delete_generated_template(&router, &address).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the in-flight generation deletable, got {status} with \
             {body}"
        );
        assert!(
            xml_text(&body, "Code").is_none(),
            "case {case:?}: expected a successful response, got {body}"
        );

        // Nothing addresses it afterwards, by either address — and the generation it was
        // discarded in the middle of never comes back once its conclusion falls due.
        assert_not_addressable(&router, case, &name).await;
        assert_not_addressable(&router, case, &id).await;
        wait_for_the_generation().await;
        assert_not_addressable(&router, case, &name).await;
        assert_not_addressable(&router, case, &id).await;
    }
}
