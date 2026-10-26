// SPDX-License-Identifier: CC0-1.0

//! `ListGeneratedTemplates`, end to end over the awsQuery wire.
//!
//! RULE: `ListGeneratedTemplates` reports one `TemplateSummary` per generated template the
//! account and Region the request is signed for holds, in creation order and one bounded
//! page at a time, each summary projecting the SAME stored record `DescribeGeneratedTemplate`
//! reports in full — its ARN and name, the status its generation has reached with that
//! status's reason, the instants it was created and last updated, and how many resources it
//! covers. A Region holding none is answered an empty list rather than a refusal, a record
//! of another Region is not listed here, and listing a generation neither advances it nor
//! retires it from the in-flight set the creation quota meters.
//!
//! The rows of each table differ along what the rule generalises over rather than along one
//! recorded case: how many resources a record covers (none, one, two, several), where those
//! resources came from (a base stack, the request's own list, both), how many records the
//! Region holds (none, one, several, more than a page), the Region the request is signed
//! for, and the page size the request asks for — at each published bound of `MaxResults`
//! (`@range(min: 1, max: 100)`) and one past it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, generated_template as generated,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    assert_generated_template_id_shape, create_generated_template, deploy,
    describe_generated_template, error_code, listed_members, wait_for_the_generation,
    ListedResource,
};
use crate::stack_lifecycle::{
    call, call_in, member, router, router_with_engine, store, xml_blocks, xml_text, REGION,
};
use crate::stack_resources::{element, is_iso8601_instant};

/// A base stack declaring one resource.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring three resources of two types.
const THREE_RESOURCE_STACK: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Beta":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// One `TemplateSummary` as the response carries it.
///
/// Read member by member inside its own `<member>` block, so each summary's name stays
/// paired with its own id, status, timestamps and resource count.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireSummary {
    generated_template_id: Option<String>,
    generated_template_name: Option<String>,
    status: Option<String>,
    status_reason: Option<String>,
    creation_time: Option<String>,
    last_updated_time: Option<String>,
    number_of_resources: Option<String>,
}

/// Every `TemplateSummary` in a `ListGeneratedTemplates` response, in wire order.
fn wire_summaries(xml: &str) -> Vec<WireSummary> {
    let Some(block) = xml_blocks(xml, "Summaries").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .iter()
        .map(|entry| WireSummary {
            generated_template_id: element(entry, "GeneratedTemplateId"),
            generated_template_name: element(entry, "GeneratedTemplateName"),
            status: element(entry, "Status"),
            status_reason: element(entry, "StatusReason"),
            creation_time: element(entry, "CreationTime"),
            last_updated_time: element(entry, "LastUpdatedTime"),
            number_of_resources: element(entry, "NumberOfResources"),
        })
        .collect()
}

/// The names a response lists, in wire order.
fn listed_names(summaries: &[WireSummary]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary.generated_template_name.clone().unwrap_or_default())
        .collect()
}

async fn list_generated_templates(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "ListGeneratedTemplates", members).await
}

/// One row of the projection table: a generated template to create, and how many resources
/// the record it leaves behind covers.
struct ListedTemplate {
    case: &'static str,
    /// The base stack's template, or `None` for a request that names no base stack.
    template: Option<&'static str>,
    /// The resources the request lists itself.
    listed: &'static [ListedResource],
    /// `TemplateConfiguration`'s two policies, when the request supplies them.
    configuration: Option<(&'static str, &'static str)>,
    /// How many resources the record covers, which is what `NumberOfResources` reports.
    resources: usize,
}

/// The rows of the projection table: no resources at all, one, two and four; taken from a
/// base stack, listed by the request itself, and both at once; with a template
/// configuration and without.
///
/// Four rows, so the whole table is created inside the in-flight quota
/// ([`generated::MAX_CONCURRENT_GENERATIONS`]) and the listing it is read through holds
/// several records at once.
const LISTED_TEMPLATES: &[ListedTemplate] = &[
    ListedTemplate {
        case: "a generation covering no resource at all",
        template: None,
        listed: &[],
        configuration: None,
        resources: 0,
    },
    ListedTemplate {
        case: "one resource, from a base stack",
        template: Some(ONE_RESOURCE_STACK),
        listed: &[],
        configuration: None,
        resources: 1,
    },
    ListedTemplate {
        case: "two resources the request lists itself, with a configuration",
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
        resources: 2,
    },
    ListedTemplate {
        case: "three resources from a base stack plus one the request lists",
        template: Some(THREE_RESOURCE_STACK),
        listed: &[ListedResource {
            resource_type: "AWS::S3::Bucket",
            logical_resource_id: "ListedBucket",
            identifier_key: "BucketName",
            identifier_value: "a-listed-bucket",
        }],
        configuration: Some(("RETAIN", "DELETE")),
        resources: 4,
    },
];

/// The request members that create the row's generated template.
fn creation_members(index: usize, row: &ListedTemplate) -> Vec<(String, String)> {
    let mut extra: Vec<(String, String)> = Vec::new();
    if row.template.is_some() {
        extra.push(member("StackName", &format!("listed-base-{index}")));
    }
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
    extra
}

/// The sum of the `TemplateProgress` counts a `DescribeGeneratedTemplate` body reports,
/// which is every resource the record covers whatever state its generation is in.
fn described_resource_count(case: &str, body: &str) -> usize {
    ["ResourcesSucceeded", "ResourcesFailed", "ResourcesProcessing", "ResourcesPending"]
        .iter()
        .map(|tag| {
            xml_text(body, tag)
                .unwrap_or_else(|| panic!("case {case:?}: expected a {tag}, got {body}"))
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("case {case:?}: expected a count in {tag}, got {body}"))
        })
        .sum()
}

/// Every record the Region holds is listed, in creation order, projecting the record the
/// full read reports.
#[tokio::test]
async fn every_generated_template_of_the_region_is_listed_as_its_record() {
    let router = router();

    // A Region holding no generated template is answered an EMPTY list, not a refusal.
    let (status, body) = list_generated_templates(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"a Region holding none\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        Vec::<String>::new(),
        "case \"a Region holding none\": expected no summary, got {body}"
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "case \"a Region holding none\": a single-page answer carries no NextToken, got {body}"
    );

    // Create the table's records one by one, listing after each: the listing grows by
    // exactly the record just created, at the END of it, which is creation order.
    let mut names: Vec<String> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (index, row) in LISTED_TEMPLATES.iter().enumerate() {
        let case = row.case;
        let name = format!("listed-template-{index}");
        if let Some(template) = row.template {
            deploy(&router, &format!("listed-base-{index}"), template).await;
        }
        let (status, body) =
            create_generated_template(&router, &name, &creation_members(index, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
        assert_generated_template_id_shape(case, &id);
        names.push(name);
        ids.push(id);

        let (status, body) = list_generated_templates(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListGeneratedTemplates answered {status} with {body}"
        );
        assert_eq!(
            listed_names(&wire_summaries(&body)),
            names,
            "case {case:?}: expected every record so far, in creation order, got {body}"
        );
    }

    // Past the due instant every generation has concluded, so the listing and the full read
    // are both reporting a settled record and can be compared member by member.
    wait_for_the_generation().await;
    let (status, body) = list_generated_templates(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the settled listing\": expected 200, got {status} with {body}"
    );
    let summaries = wire_summaries(&body);
    assert_eq!(
        listed_names(&summaries),
        names,
        "case \"the settled listing\": expected every record, in creation order, got {body}"
    );
    assert_eq!(
        xml_text(&body, "NextToken"),
        None,
        "case \"the settled listing\": every record fits one page, got {body}"
    );

    for (index, row) in LISTED_TEMPLATES.iter().enumerate() {
        let case = row.case;
        let summary = &summaries[index];
        assert_eq!(
            summary.generated_template_id.as_deref(),
            Some(ids[index].as_str()),
            "case {case:?}: expected the id the create minted, got {summary:?}"
        );
        assert_eq!(
            summary.number_of_resources.as_deref(),
            Some(row.resources.to_string().as_str()),
            "case {case:?}: expected {} covered resources, got {summary:?}",
            row.resources
        );
        assert_eq!(
            summary.status.as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected the concluded generation's status, got {summary:?}"
        );
        assert_eq!(
            summary.status_reason.as_deref(),
            Some(generated::GENERATION_COMPLETE_REASON),
            "case {case:?}: expected the concluded generation's reason, got {summary:?}"
        );
        for (tag, instant) in [
            ("CreationTime", summary.creation_time.as_deref()),
            ("LastUpdatedTime", summary.last_updated_time.as_deref()),
        ] {
            let instant = instant
                .unwrap_or_else(|| panic!("case {case:?}: expected a {tag}, got {summary:?}"));
            assert!(
                is_iso8601_instant(instant),
                "case {case:?}: expected {tag} as an ISO-8601 instant, got {instant:?}"
            );
        }

        // The summary PROJECTS the record the full read reports: the same id, name, status,
        // reason and instants, and a resource count equal to the progress the read counts
        // over that record's resources.
        let (status, described) = describe_generated_template(&router, &names[index]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {described}"
        );
        for tag in [
            "GeneratedTemplateId",
            "GeneratedTemplateName",
            "Status",
            "StatusReason",
            "CreationTime",
            "LastUpdatedTime",
        ] {
            let listed = match tag {
                "GeneratedTemplateId" => summary.generated_template_id.clone(),
                "GeneratedTemplateName" => summary.generated_template_name.clone(),
                "Status" => summary.status.clone(),
                "StatusReason" => summary.status_reason.clone(),
                "CreationTime" => summary.creation_time.clone(),
                "LastUpdatedTime" => summary.last_updated_time.clone(),
                other => panic!("{other} is not a member of a TemplateSummary"),
            };
            assert_eq!(
                listed,
                xml_text(&described, tag),
                "case {case:?}: expected the listed {tag} to be the record's, got {described}"
            );
        }
        assert_eq!(
            summary.number_of_resources.as_deref(),
            Some(described_resource_count(case, &described).to_string().as_str()),
            "case {case:?}: expected the count the record's progress totals, got {described}"
        );
    }
}

/// A generation still in flight is listed as in flight, and listing it does not conclude
/// it or retire it from the set the creation quota meters.
#[tokio::test]
async fn a_generation_in_flight_is_listed_in_flight_and_stays_in_flight() {
    let router = router();
    let cap = generated::MAX_CONCURRENT_GENERATIONS;
    let case = "a full in-flight set, listed";

    let started = Instant::now();
    let names: Vec<String> = (0..cap).map(|index| format!("in-flight-{index}")).collect();
    for name in &names {
        let (status, body) = create_generated_template(&router, name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate({name:?}) answered {status} with {body}"
        );
    }

    // List the whole set repeatedly. A listing is not a transition: each one answers the
    // records as the clock left them.
    let mut listings: Vec<Vec<WireSummary>> = Vec::new();
    for _ in 0..3 {
        let (status, body) = list_generated_templates(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListGeneratedTemplates answered {status} with {body}"
        );
        listings.push(wire_summaries(&body));
    }
    if started.elapsed() < generated::GENERATION_DELAY {
        let first = &listings[0];
        for (round, listing) in listings.iter().enumerate() {
            assert_eq!(
                listing, first,
                "case {case:?}: expected listing {round} to report the records unchanged, got \
                 {listing:?} after {first:?}"
            );
        }
        for summary in first {
            assert_eq!(
                summary.status.as_deref(),
                Some(generated::status::CREATE_IN_PROGRESS),
                "case {case:?}: expected the generation still in flight, got {summary:?}"
            );
            assert_eq!(
                summary.status_reason, None,
                "case {case:?}: a generation in flight carries no reason yet, got {summary:?}"
            );
        }
        // And the listings did not retire any of them from the set the quota meters: the
        // cap is still full.
        let (status, body) = create_generated_template(&router, "one-too-many", &[]).await;
        assert_eq!(
            status, 429,
            "case {case:?}: expected the in-flight set still full after the listings, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ConcurrentResourcesLimitExceeded",
            "case {case:?}: expected the modelled concurrency refusal, got {body}"
        );
    }

    // Past the due instant the same listing reports the conclusion the clock owed each
    // record, and the records it listed are the same ones.
    wait_for_the_generation().await;
    let (status, body) = list_generated_templates(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListGeneratedTemplates answered {status} with {body}"
    );
    let settled = wire_summaries(&body);
    assert_eq!(
        listed_names(&settled),
        names,
        "case {case:?}: expected the same records, in creation order, got {body}"
    );
    for summary in &settled {
        assert_eq!(
            summary.status.as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected the generation concluded, got {summary:?}"
        );
        assert_eq!(
            summary.status_reason.as_deref(),
            Some(generated::GENERATION_COMPLETE_REASON),
            "case {case:?}: expected the concluded generation's reason, got {summary:?}"
        );
        assert_eq!(
            summary.number_of_resources.as_deref(),
            Some("0"),
            "case {case:?}: these generations cover no resource, got {summary:?}"
        );
    }
}

/// `ListGeneratedTemplates` lists "your generated templates in this Region": the same name
/// in a second Region is a second record, and each Region's listing reports only its own.
#[tokio::test]
async fn a_generated_template_is_listed_only_in_the_region_it_was_created_in() {
    let router = router();
    // One name created in several Regions, plus a Region that holds nothing of its own.
    let shared = "regional-template";
    let regions = [REGION, "eu-west-1", "ap-southeast-2"];
    let mut own: Vec<(&str, Vec<String>)> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let mut names = vec![shared.to_string()];
        // A second record in every Region past the first, so a Region's listing is not
        // merely the same single row everywhere.
        for extra in 0..index {
            names.push(format!("regional-template-{region}-{extra}"));
        }
        for name in &names {
            let (status, body) = call_in(
                &router,
                region,
                "CreateGeneratedTemplate",
                &[member("GeneratedTemplateName", name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {region:?}: CreateGeneratedTemplate({name:?}) answered {status} with {body}"
            );
        }
        own.push((region, names));
    }

    for (region, names) in &own {
        let (status, body) = call_in(&router, region, "ListGeneratedTemplates", &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: ListGeneratedTemplates answered {status} with {body}"
        );
        let summaries = wire_summaries(&body);
        assert_eq!(
            listed_names(&summaries),
            *names,
            "case {region:?}: expected only this Region's records, got {body}"
        );
        for summary in &summaries {
            let id = summary
                .generated_template_id
                .clone()
                .unwrap_or_else(|| panic!("case {region:?}: expected an id, got {summary:?}"));
            assert!(
                id.contains(&format!(":cloudformation:{region}:")),
                "case {region:?}: expected the record's own Region in its ARN, got {id:?}"
            );
        }
    }

    // A Region that created nothing lists nothing, though other Regions hold records.
    let (status, body) = call_in(&router, "ca-central-1", "ListGeneratedTemplates", &[]).await;
    assert_eq!(
        status, 200,
        "case \"a Region that created nothing\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&body)),
        Vec::<String>::new(),
        "case \"a Region that created nothing\": expected no summary, got {body}"
    );
}

/// How many records the paginated cases seed: more than one default page, so the default
/// read must hand out a token and the resumed read must finish the collection.
const PAGINATED_TEMPLATES: usize = generated::DEFAULT_LISTED_TEMPLATES + 23;

/// The name of the `index`th seeded record.
fn paginated_name(index: usize) -> String {
    format!("paged-template-{index:03}")
}

/// The router with [`PAGINATED_TEMPLATES`] generated templates in `REGION`, in creation
/// order — more than the in-flight quota's worth of wire calls could create at once.
///
/// Seeded with their generation already due, so the first listing settles every one of them
/// and the pages a walk reads are stable.
fn router_with_paginated_templates() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let long_ago = SystemTime::now() - generated::GENERATION_DELAY - Duration::from_secs(1);
        for index in 0..PAGINATED_TEMPLATES {
            let name = paginated_name(index);
            let mut record = generated::StoredGeneratedTemplate::starting(
                state::new_generated_template_id(REGION, &name),
                name,
                REGION.to_string(),
                long_ago,
            );
            // A varying resource count, so a page is not a run of identical summaries.
            for position in 0..index % 3 {
                record.resources.push(generated::GeneratedResource {
                    resource_type: "AWS::SNS::Topic".to_string(),
                    logical_resource_id: Some(format!("Topic{position}")),
                    resource_identifier: BTreeMap::new(),
                });
            }
            held.generated_templates.push(record);
        }
    }
    router
}

#[tokio::test]
async fn the_listing_is_paged_and_a_token_resumes_just_past_the_page() {
    let router = router_with_paginated_templates();
    let expected: Vec<String> = (0..PAGINATED_TEMPLATES).map(paginated_name).collect();
    let default_page = generated::DEFAULT_LISTED_TEMPLATES;

    // A request naming no `MaxResults` gets the documented default page size, and a token.
    let (status, first) = list_generated_templates(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the default page\": expected 200, got {status} with {first}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&first)),
        expected[..default_page],
        "case \"the default page\": the first {default_page} records, got {first}"
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the default page\": expected a NextToken, got {first}")
    });

    let (status, second) = list_generated_templates(&router, &[member("NextToken", &token)]).await;
    assert_eq!(
        status, 200,
        "case \"the resumed page\": expected 200, got {status} with {second}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&second)),
        expected[default_page..],
        "case \"the resumed page\": the rest of the collection, got {second}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no token, got {second}"
    );

    // `MaxResults` is "the maximum number of results to be returned with a single call": a
    // walk in pages of that size reports the whole collection exactly once, in order, and a
    // caller may ask for a different maximum on every call.
    for max_results in [1usize, 7, default_page, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut counts: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut members = vec![member("MaxResults", &max_results.to_string())];
            if let Some(token) = &next {
                members.push(member("NextToken", token));
            }
            let (status, body) = list_generated_templates(&router, &members).await;
            assert_eq!(
                status, 200,
                "case \"MaxResults {max_results}\": expected 200, got {status} with {body}"
            );
            let summaries = wire_summaries(&body);
            assert!(
                summaries.len() <= max_results,
                "case \"MaxResults {max_results}\": a page may not exceed the maximum, got {} \
                 in {body}",
                summaries.len()
            );
            walked.extend(listed_names(&summaries));
            counts.extend(
                summaries
                    .iter()
                    .map(|summary| summary.number_of_resources.clone().unwrap_or_default()),
            );
            next = xml_text(&body, "NextToken");
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case \"MaxResults {max_results}\": the walk must report the collection once, got \
             {walked:?}"
        );
        let seeded: Vec<String> = (0..PAGINATED_TEMPLATES)
            .map(|index| (index % 3).to_string())
            .collect();
        assert_eq!(
            counts, seeded,
            "case \"MaxResults {max_results}\": each summary carries its own record's resource \
             count, got {counts:?}"
        );
        let distinct: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            distinct.len(),
            PAGINATED_TEMPLATES,
            "case \"MaxResults {max_results}\": no record is listed twice, got {} of \
             {PAGINATED_TEMPLATES}",
            distinct.len()
        );
    }
}

/// A `MaxResults` outside the modelled range is refused, at each published bound and one
/// past it.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let router = router_with_paginated_templates();

    // `MaxResults` is `@range(min: 1, max: 100)`.
    for (case, max_results, accepted) in [
        ("the published minimum", "1", true),
        ("below the published minimum", "0", false),
        ("the published maximum", "100", true),
        ("past the published maximum", "101", false),
    ] {
        let (status, body) =
            list_generated_templates(&router, &[member("MaxResults", max_results)]).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={max_results} is within the range, got {status} with \
                 {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={max_results} is outside the range, got {status} with \
                 {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected the constraint refusal, got {body}"
            );
        }
    }
}

/// A `NextToken` this listing never issued is a bad request, not an empty page — including
/// one this service issued for ANOTHER collection.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let router = router_with_paginated_templates();

    let past_the_end =
        state::encode_next_token(state::PageOf::TemplateSummaries, PAGINATED_TEMPLATES + 1);
    let another_collection = state::encode_next_token(state::PageOf::StackSummaries, 0);
    for (case, token) in [
        ("a cursor past the end of the listing", past_the_end),
        ("a cursor issued for another collection", another_collection),
        ("a token this service never issued", "not-a-token".to_string()),
    ] {
        let (status, body) =
            list_generated_templates(&router, &[member("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("{token} is not a valid NextToken").as_str()),
            "case {case:?}: expected the invalid-token refusal, got {body}"
        );
    }

    // The cursor at the START of this collection IS accepted, so the refusals above are
    // about the token and not about the operation.
    let first = state::encode_next_token(state::PageOf::TemplateSummaries, 0);
    let (status, body) = list_generated_templates(&router, &[member("NextToken", &first)]).await;
    assert_eq!(
        status, 200,
        "case \"a cursor at the start of this collection\": expected 200, got {status} with {body}"
    );
    assert_eq!(
        listed_names(&wire_summaries(&body)).len(),
        generated::DEFAULT_LISTED_TEMPLATES,
        "case \"a cursor at the start of this collection\": expected the first page, got {body}"
    );
}
