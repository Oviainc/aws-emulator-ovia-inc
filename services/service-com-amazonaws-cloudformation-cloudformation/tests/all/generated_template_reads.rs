// SPDX-License-Identifier: CC0-1.0

//! `DescribeGeneratedTemplate` as a READ, over the awsQuery wire: WHICH record a request
//! addresses, and that describing a generation reports that record rather than changing it.
//!
//! RULE: `DescribeGeneratedTemplate` answers from the generated-template record its
//! `GeneratedTemplateName` addresses — "the name or Amazon Resource Name (ARN) of a
//! generated template" — within the account and Region the request is signed for; an
//! address no record of that Region holds is refused 404 `GeneratedTemplateNotFound`; and
//! because the read only reports, every value it answers comes from the record and the only
//! thing that moves between two reads is the generation conclusion the clock had already
//! made due.
//!
//! Three suites, one per face of that rule, each a table whose rows differ along what the
//! rule generalises over rather than along one recorded case:
//!
//! 1. **Addressing.** By name and by the record's own ARN, over generations covering no
//!    resource at all, one, two and three, taken from a base stack and listed by the
//!    request itself, under names spanning `GeneratedTemplateName`'s own
//!    `@length(min: 1, max: 128)`. Either address answers the SAME record, and reports the
//!    record's name even when the request named its ARN.
//! 2. **The Region the request is signed for.** The same name in a second Region is a
//!    second generated template, each Region's read resolves its own, and neither the other
//!    Region's name nor its ARN is addressable from here.
//! 3. **The read reports.** Polling a generation neither advances it nor retires it from
//!    the in-flight set the creation quota meters, and a refused read persists nothing.
//!
//! The generation covering NO resource is the row that pins the modelled
//! `ResourceDetails`'s `@length(min: 1)`: a list that would be empty is reported absent
//! rather than as an empty list.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    assert_generated_template_id_shape, create_generated_template, deploy,
    describe_generated_template, error_code, listed_members, wait_for_the_generation,
    ListedResource,
};
use crate::stack_lifecycle::{call_in, member, router, xml_blocks, xml_text, xml_texts, REGION};

/// A base stack declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring two resources of two types.
const TWO_RESOURCE_STACK: &str = r#"{"Resources":{
    "Bucket":{"Type":"AWS::S3::Bucket"},
    "Table":{"Type":"AWS::DynamoDB::Table"}
}}"#;

/// `CreateGeneratedTemplate` signed for `region`.
async fn create_generated_template_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("GeneratedTemplateName", name)];
    members.extend_from_slice(extra);
    call_in(router, region, "CreateGeneratedTemplate", &members).await
}

/// `DescribeGeneratedTemplate` signed for `region`: a generated template belongs to the
/// Region the call that created it was signed for, so the credential scope of the read is
/// what decides which Region's records it can address.
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

/// What one read reported of the stored record: its status, its two stamps and the count
/// of the resources the generation succeeded on.
type ReportedRecord = (Option<String>, Option<String>, Option<String>, Option<String>);

/// The four members of a described generation that come from the stored record, as one
/// tuple, so two reads can be compared for having answered the same record.
fn reported_record(body: &str) -> ReportedRecord {
    (
        xml_text(body, "Status"),
        xml_text(body, "CreationTime"),
        xml_text(body, "LastUpdatedTime"),
        xml_text(body, "ResourcesSucceeded"),
    )
}

/// The `LogicalResourceId`s a described generation reports, sorted.
fn reported_logical_ids(body: &str) -> Vec<String> {
    let mut ids = xml_texts(body, "LogicalResourceId");
    ids.sort();
    ids
}

/// One row of the addressing table.
struct ReadCase {
    case: &'static str,
    /// The name the generation is created under. The shapes span the member's own
    /// constraint, `GeneratedTemplateName` `@length(min: 1, max: 128)`.
    name: String,
    /// The base stack's template, or `None` for a generation given no base stack.
    template: Option<&'static str>,
    /// The resources the request lists itself.
    listed: Vec<ListedResource>,
    /// The logical ids the generation must report, sorted.
    logical_ids: Vec<&'static str>,
}

/// The rows: no covered resource at all, one, two and three; from a base stack, listed by
/// the request, and both; and names at both ends of the modelled length range.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            // `@length(min: 1)`: the shortest name the member allows, on the generation
            // that covers nothing at all.
            case: "a one-character name, no covered resource at all",
            name: "a".to_string(),
            template: None,
            listed: Vec::new(),
            logical_ids: Vec::new(),
        },
        ReadCase {
            // `@length(max: 128)`: the longest name the member allows.
            case: "a 128-character name, one resource from a base stack",
            name: format!("generated-template-{}", "x".repeat(128 - "generated-template-".len())),
            template: Some(ONE_RESOURCE_STACK),
            listed: Vec::new(),
            logical_ids: vec!["Topic"],
        },
        ReadCase {
            case: "a mixed-case dotted name, two resources the request lists itself",
            name: "Read.Me_2-Generated".to_string(),
            template: None,
            listed: vec![
                ListedResource {
                    resource_type: "AWS::SNS::Topic",
                    logical_resource_id: "ReadTopic",
                    identifier_key: "TopicArn",
                    identifier_value: "arn:aws:sns:us-east-1:000000000000:a-read-topic",
                },
                ListedResource {
                    resource_type: "AWS::DynamoDB::Table",
                    logical_resource_id: "ReadTable",
                    identifier_key: "TableName",
                    identifier_value: "a-read-table",
                },
            ],
            logical_ids: vec!["ReadTable", "ReadTopic"],
        },
        ReadCase {
            case: "two resources from a base stack plus one the request lists itself",
            name: "three-covered-resources".to_string(),
            template: Some(TWO_RESOURCE_STACK),
            listed: vec![ListedResource {
                resource_type: "AWS::Logs::LogGroup",
                logical_resource_id: "ReadGroup",
                identifier_key: "LogGroupName",
                identifier_value: "a-read-group",
            }],
            logical_ids: vec!["Bucket", "ReadGroup", "Table"],
        },
    ]
}

/// A generated template is addressable by its NAME and by its own ARN, and either address
/// answers the same stored record — including the record's name, which a read addressed by
/// ARN never saw in its request.
#[tokio::test]
async fn a_generated_template_is_addressable_by_name_and_by_its_arn() {
    let router = router();

    for (index, row) in read_cases().into_iter().enumerate() {
        let case = row.case;
        let mut extra: Vec<(String, String)> = Vec::new();
        if let Some(template) = row.template {
            let stack = format!("read-base-stack-{index}");
            deploy(&router, &stack, template).await;
            extra.push(member("StackName", &stack));
        }
        for (position, listed) in row.listed.iter().enumerate() {
            extra.extend(listed_members(position, listed));
        }

        let started = Instant::now();
        let (status, body) = create_generated_template(&router, &row.name, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
        );
        let id = xml_text(&body, "GeneratedTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
        assert_generated_template_id_shape(case, &id);

        // Both addresses, while the generation is still in flight: the same record either
        // way, and the record's own name even for the read that named the ARN.
        let mut in_flight_reads = Vec::new();
        for address in [row.name.clone(), id.clone()] {
            let (status, body) = describe_generated_template(&router, &address).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeGeneratedTemplate({address:?}) answered {status} with \
                 {body}"
            );
            assert_eq!(
                xml_text(&body, "GeneratedTemplateId").as_deref(),
                Some(id.as_str()),
                "case {case:?}: expected {address:?} to address the created record, got {body}"
            );
            assert_eq!(
                xml_text(&body, "GeneratedTemplateName").as_deref(),
                Some(row.name.as_str()),
                "case {case:?}: expected the record's own name for {address:?}, got {body}"
            );
            in_flight_reads.push(body);
        }
        if started.elapsed() < generated::GENERATION_DELAY {
            for body in &in_flight_reads {
                assert_eq!(
                    xml_text(body, "Status").as_deref(),
                    Some(generated::status::CREATE_IN_PROGRESS),
                    "case {case:?}: expected the generation still in flight, got {body}"
                );
                // Nothing has updated the record since it was created, so the two stamps
                // the record carries are still the one instant it was created at.
                assert_eq!(
                    xml_text(body, "LastUpdatedTime"),
                    xml_text(body, "CreationTime"),
                    "case {case:?}: expected an untouched record's stamps to agree, got {body}"
                );
            }
            assert_eq!(
                reported_record(&in_flight_reads[0]),
                reported_record(&in_flight_reads[1]),
                "case {case:?}: expected the name and the ARN to answer one record, got {:?} and \
                 {:?}",
                in_flight_reads[0],
                in_flight_reads[1]
            );
        }

        // And once the clock has made the conclusion due, both addresses report it.
        wait_for_the_generation().await;
        let mut settled_reads = Vec::new();
        for address in [row.name.clone(), id.clone()] {
            let (status, body) = describe_generated_template(&router, &address).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeGeneratedTemplate({address:?}) answered {status} with \
                 {body}"
            );
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(generated::status::COMPLETE),
                "case {case:?}: expected the generation complete for {address:?}, got {body}"
            );
            assert_eq!(
                reported_logical_ids(&body),
                row.logical_ids,
                "case {case:?}: expected the covered resources for {address:?}, got {body}"
            );
            assert_eq!(
                xml_text(&body, "ResourcesSucceeded"),
                Some(row.logical_ids.len().to_string()),
                "case {case:?}: expected every covered resource succeeded, got {body}"
            );
            assert_eq!(
                xml_text(&body, "TotalWarnings").as_deref(),
                Some("0"),
                "case {case:?}: expected no warnings, got {body}"
            );
            // The conclusion IS an update of the record, so the record's two stamps no
            // longer agree.
            assert_ne!(
                xml_text(&body, "LastUpdatedTime"),
                xml_text(&body, "CreationTime"),
                "case {case:?}: expected the conclusion to have updated the record, got {body}"
            );
            // `ResourceDetails` is `@length(min: 1)`: a generation covering no resource
            // reports no list at all rather than an empty one.
            if row.logical_ids.is_empty() {
                assert!(
                    xml_blocks(&body, "Resources").is_empty(),
                    "case {case:?}: expected no Resources list for a generation covering \
                     nothing, got {body}"
                );
                assert!(
                    xml_texts(&body, "ResourceStatus").is_empty(),
                    "case {case:?}: expected no resource rows, got {body}"
                );
            }
            settled_reads.push(body);
        }
        assert_eq!(
            reported_record(&settled_reads[0]),
            reported_record(&settled_reads[1]),
            "case {case:?}: expected the name and the ARN to answer one record, got {:?} and {:?}",
            settled_reads[0],
            settled_reads[1]
        );
    }
}

/// The record a read can address is the one its own Region holds: the same name in a second
/// Region is a second generated template, and neither the other Region's name nor its ARN
/// is addressable from here.
#[tokio::test]
async fn a_generated_template_is_addressable_only_in_the_region_it_was_created_in() {
    let router = router();
    // Pairs of Regions, one of which is the suite's default and one in which neither is, so
    // nothing about the rule hangs on a privileged Region.
    let pairs = [(REGION, "eu-west-2"), ("ap-northeast-1", "sa-east-1")];
    let name = "the-same-name-everywhere";

    for (first, second) in pairs {
        let case = format!("{name:?} created in both {first} and {second}");
        let mut ids = Vec::new();
        for (region, listed) in [
            (
                first,
                vec![ListedResource {
                    resource_type: "AWS::SNS::Topic",
                    logical_resource_id: "RegionalTopic",
                    identifier_key: "TopicArn",
                    identifier_value: "arn:aws:sns:us-east-1:000000000000:a-regional-topic",
                }],
            ),
            (second, Vec::new()),
        ] {
            let mut extra: Vec<(String, String)> = Vec::new();
            for (position, resource) in listed.iter().enumerate() {
                extra.extend(listed_members(position, resource));
            }
            let (status, body) = create_generated_template_in(&router, region, name, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateGeneratedTemplate in {region} answered {status} with {body}"
            );
            let id = xml_text(&body, "GeneratedTemplateId").unwrap_or_else(|| {
                panic!("case {case:?}: expected a GeneratedTemplateId in {region}, got {body}")
            });
            assert!(
                id.contains(&format!(":{region}:")),
                "case {case:?}: expected the record's ARN to carry {region}, got {id:?}"
            );
            ids.push(id);
        }
        assert_ne!(
            ids[0], ids[1],
            "case {case:?}: expected two records, got one id {:?}",
            ids[0]
        );
        wait_for_the_generation().await;

        for (region, own_id, other_id) in [
            (first, &ids[0], &ids[1]),
            (second, &ids[1], &ids[0]),
        ] {
            // The name resolves THAT Region's record.
            let (status, body) = describe_generated_template_in(&router, region, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read in {region} answered {status} with {body}"
            );
            assert_eq!(
                xml_text(&body, "GeneratedTemplateId").as_deref(),
                Some(own_id.as_str()),
                "case {case:?}: expected {region} to report its own record, got {body}"
            );
            // ...and the other Region's ARN addresses nothing here.
            let (status, body) = describe_generated_template_in(&router, region, other_id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the other Region's ARN unknown in {region}, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(&case, &body),
                "GeneratedTemplateNotFound",
                "case {case:?}: expected the modelled not-found refusal in {region}, got {body}"
            );
        }

        // A third Region holds no record of that name at all.
        let (status, body) = describe_generated_template_in(&router, "ca-central-1", name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the name unknown in a Region that holds nothing, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&case, &body),
            "GeneratedTemplateNotFound",
            "case {case:?}: expected the modelled not-found refusal, got {body}"
        );
    }
}

/// The read REPORTS: polling a generation does not advance it, does not retire it from the
/// in-flight set the creation quota meters, and a refused read leaves nothing behind.
#[tokio::test]
async fn describing_a_generation_changes_nothing_a_later_caller_sees() {
    let router = router();
    let cap = generated::MAX_CONCURRENT_GENERATIONS;
    let case = "polling every generation of a full in-flight set";

    // A refused read persists nothing: the name it could not resolve is still free.
    let (status, body) = describe_generated_template(&router, "polled-0").await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the unknown name refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "GeneratedTemplateNotFound",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );

    let started = Instant::now();
    let names: Vec<String> = (0..cap).map(|index| format!("polled-{index}")).collect();
    for name in &names {
        let (status, body) = create_generated_template(&router, name, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGeneratedTemplate({name:?}) answered {status} with {body}"
        );
    }

    // Poll each of them repeatedly. A read is not a transition: every poll answers the
    // record as the clock left it.
    let mut polled: Vec<Vec<ReportedRecord>> = vec![Vec::new(); names.len()];
    for _ in 0..3 {
        for (position, name) in names.iter().enumerate() {
            let (status, body) = describe_generated_template(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeGeneratedTemplate({name:?}) answered {status} with {body}"
            );
            polled[position].push(reported_record(&body));
        }
    }
    if started.elapsed() < generated::GENERATION_DELAY {
        for (position, reads) in polled.iter().enumerate() {
            let first = &reads[0];
            for (round, read) in reads.iter().enumerate() {
                assert_eq!(
                    read, first,
                    "case {case:?}: expected poll {round} of {:?} to report the record \
                     unchanged, got {read:?} after {first:?}",
                    names[position]
                );
            }
            assert_eq!(
                first.0.as_deref(),
                Some(generated::status::CREATE_IN_PROGRESS),
                "case {case:?}: expected {:?} still in flight after being polled, got {first:?}",
                names[position]
            );
        }
        // And the polls did not retire any of them from the set the quota meters: the cap
        // is still full.
        let (status, body) = create_generated_template(&router, "one-too-many", &[]).await;
        assert_eq!(
            status, 429,
            "case {case:?}: expected the in-flight set still full after the polls, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ConcurrentResourcesLimitExceeded",
            "case {case:?}: expected the modelled concurrency refusal, got {body}"
        );
    }

    // Past the due instant the records report the conclusion the clock owed them — and
    // then two further reads report that same record, however often it is read.
    wait_for_the_generation().await;
    for name in &names {
        let (status, first) = describe_generated_template(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate({name:?}) answered {status} with {first}"
        );
        assert_eq!(
            xml_text(&first, "Status").as_deref(),
            Some(generated::status::COMPLETE),
            "case {case:?}: expected {name:?} complete, got {first}"
        );
        let (_, second) = describe_generated_template(&router, name).await;
        assert_eq!(
            reported_record(&first),
            reported_record(&second),
            "case {case:?}: expected a second read of {name:?} to report the same record, got \
             {second} after {first}"
        );
    }
}
