// SPDX-License-Identifier: CC0-1.0

//! What the two READ-ONLY discovery surfaces put on the wire: the resource scans, and the
//! extension type registry — end to end over the awsQuery wire.
//!
//! Five rules are under test here, each with its own table.
//!
//! 1. **A `ResourceScanId` outside the form the member documents is an input-constraint
//!    violation, not a lookup failure.** `ResourceScanId` is "the Amazon Resource Name (ARN)
//!    of the resource scan. The format is
//!    `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`", so a value
//!    of another shape addresses no scan that could exist: it is refused 400
//!    `ValidationError` naming the member and the pattern, ahead of the state lookup, while a
//!    WELL-FORMED id nothing started is the not-found refusal. That is what lets a caller
//!    tell a typo in the id from a scan that is not there. The table is one row per way a
//!    value can fall outside the pattern, crossed with every read of a scan that takes the
//!    member.
//!
//! 2. **A modelled `double` is written as a double**, with its fractional part — so a scan
//!    reported as a hundred per cent complete carries `100.0`, not `100`, and an SDK that
//!    round-trips the member reads the literal the service wrote. Asserted on every reading
//!    of `PercentageCompleted` a scan's life passes through, from both the item read and the
//!    listing.
//!
//! 3. **`ResourceTypes` is PRESENT, possibly empty, for a scan whose status admits it.**
//!    "Resource types are only available for scans with a `Status` set to `COMPLETE` or
//!    `FAILED`", so the status is what gates the member and the scan's rows are what fill it:
//!    an account with nothing to find reports the empty list rather than no member at all.
//!
//! 4. **A value outside an enum's set is refused with the value set in the order the SERVICE
//!    lists it.** For a registration status that is the order a registration passes through
//!    (`[IN_PROGRESS, COMPLETE, FAILED]`), which is not the order the model declares; every
//!    other enum keeps the model's declaration order, which the `ScanType` row pins.
//!
//! 5. **A registry read reports every member it has a value for.** `SourceUrl` is the
//!    repository's CLONE url, so it ends in `.git`; and
//!    `BatchDescribeTypeConfigurations` answers all three of its result lists — present, as
//!    empty elements where the batch filled none of them — with the registry's own wording
//!    for an identifier whose configuration it does not hold.
//!
//! Every value asserted is one the service answered or one derived from a constant this
//! crate publishes: the scan ids come from `StartResourceScan`, the refusal pattern from
//! `state::resource_scans::RESOURCE_SCAN_ID_PATTERN`, and the source URLs from the same rule
//! the registry derives them by.

use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::error_code;
use crate::harness::{member, router, xml_blocks, xml_text, xml_texts};
use crate::stack_lifecycle::{call, deploy, ACCOUNT_ID, REGION};

/// A stack whose resources span two services, so a scan of the account finds more than one
/// resource type.
const TOPIC_AND_BUCKET: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"read-shapes-alpha-topic"}},
    "Beta":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"read-shapes-beta-bucket"}}
}}"#;

/// Wait past the instant a started scan's conclusion becomes due.
async fn wait_for_the_scan() {
    tokio::time::sleep(scans::SCAN_DELAY + Duration::from_millis(250)).await;
}

/// Start a scan of the whole account and answer the id it was given.
async fn start_a_scan(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, "StartResourceScan", &[]).await;
    assert_eq!(
        status, 200,
        "expected StartResourceScan to succeed, got {status} {body}"
    );
    xml_text(&body, "ResourceScanId")
        .unwrap_or_else(|| panic!("expected a ResourceScanId, got {body}"))
}

// ---------------------------------------------------------------------------
// 1. The shape of a ResourceScanId
// ---------------------------------------------------------------------------

/// A well-formed `ResourceScanId` for `region`, under which nothing was started.
fn a_well_formed_id(region: &str) -> String {
    scans::resource_scan_arn(region, "00000000-0000-4000-8000-00000000000a")
}

/// One row of the id-shape table: what the request supplies, and whether that value is of the
/// documented FORM at all.
struct IdCase {
    case: &'static str,
    resource_scan_id: String,
    /// `true` where the value satisfies the documented pattern, so the read gets as far as
    /// the lookup and the refusal is the not-found one.
    well_formed: bool,
}

/// One row per way a value can fall outside
/// `arn:[a-z0-9-]+:cloudformation:[a-z0-9-]+:[0-9]{12}:resourceScan/[a-z0-9-]+` — not an ARN
/// at all, an ARN of the wrong service, Region or resource, an account segment of the wrong
/// length, an id or a Region carrying characters the class excludes, and an empty segment —
/// plus the two well-formed rows the pattern admits, so the table spans both sides of the
/// rule.
fn id_cases() -> Vec<IdCase> {
    vec![
        IdCase {
            case: "no ARN at all",
            resource_scan_id: "not-a-valid-resource-scan-id".to_string(),
            well_formed: false,
        },
        IdCase {
            case: "a bare identifier",
            resource_scan_id: "00000000-0000-0000-0000-000000000000".to_string(),
            well_formed: false,
        },
        IdCase {
            case: "an empty value",
            resource_scan_id: String::new(),
            well_formed: false,
        },
        IdCase {
            case: "another service's ARN",
            resource_scan_id: format!(
                "arn:aws:s3:{REGION}:{ACCOUNT_ID}:resourceScan/00000000-0000-4000-8000-0000000b"
            ),
            well_formed: false,
        },
        IdCase {
            case: "another resource of this service",
            resource_scan_id: format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/alpha"),
            well_formed: false,
        },
        IdCase {
            case: "an account segment of the wrong length",
            resource_scan_id: format!(
                "arn:aws:cloudformation:{REGION}:00000000000:resourceScan/0000-4000-8000-0000000c"
            ),
            well_formed: false,
        },
        IdCase {
            case: "an empty Region segment",
            resource_scan_id: format!(
                "arn:aws:cloudformation::{ACCOUNT_ID}:resourceScan/00000000-0000-4000-8000-0000d"
            ),
            well_formed: false,
        },
        IdCase {
            case: "an upper-case Region segment",
            resource_scan_id: format!(
                "arn:aws:cloudformation:US-EAST-1:{ACCOUNT_ID}:resourceScan/0000-4000-8000-0000e"
            ),
            well_formed: false,
        },
        IdCase {
            case: "an upper-case id segment",
            resource_scan_id: format!(
                "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:resourceScan/ABCDEF00-0000-4000-8"
            ),
            well_formed: false,
        },
        IdCase {
            case: "an empty id segment",
            resource_scan_id: format!(
                "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:resourceScan/"
            ),
            well_formed: false,
        },
        IdCase {
            case: "a value far longer than a refusal may quote back",
            resource_scan_id: "z".repeat(MAX_ECHOED_VALUE * 4),
            well_formed: false,
        },
        IdCase {
            case: "a well-formed id nothing was started under",
            resource_scan_id: a_well_formed_id(REGION),
            well_formed: true,
        },
        IdCase {
            case: "a well-formed id of another Region",
            resource_scan_id: a_well_formed_id("eu-west-3"),
            well_formed: true,
        },
    ]
}

/// Ceiling on the value a refusal quotes back, and the elision past it.
///
/// Source: the boundary validator's own (`MAX_ECHOED_VALUE` in
/// `vendor/smithy-schema-server/src/constraints.rs`, 256 bytes followed by
/// `... [<len> bytes]`), which is what a refusal this service words quotes a caller's value
/// by too — `ResourceScanId` carries no modelled `@length`, so a request could otherwise
/// make the refusal as long as the value it sent.
const MAX_ECHOED_VALUE: usize = 256;

/// `supplied` as the refusal quotes it.
fn echoed(supplied: &str) -> String {
    if supplied.len() <= MAX_ECHOED_VALUE {
        return supplied.to_string();
    }
    format!(
        "{}... [{} bytes]",
        &supplied[..MAX_ECHOED_VALUE],
        supplied.len()
    )
}

/// The refusal CloudFormation's input validation answers for `supplied`: the member it names,
/// and the pattern it failed, both read off the constant this crate publishes rather than
/// spelled again here.
fn expected_pattern_refusal(supplied: &str) -> String {
    format!(
        "1 validation error detected: Value '{}' at 'resourceScanId' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {}",
        echoed(supplied),
        scans::RESOURCE_SCAN_ID_PATTERN
    )
}

/// The request members each read of a scan needs beside the id it is about.
fn beside_the_id(action: &str) -> Vec<(String, String)> {
    if action == "ListResourceScanRelatedResources" {
        // `Resources` is `@required` on this read, so a case about the ID still has to carry
        // one addressed resource for the request to be well-formed at all.
        return vec![
            member("Resources.member.1.ResourceType", "AWS::SNS::Topic"),
            member(
                "Resources.member.1.ResourceIdentifier.entry.1.key",
                "TopicArn",
            ),
            member(
                "Resources.member.1.ResourceIdentifier.entry.1.value",
                "arn:aws:sns:us-east-1:000000000000:read-shapes-alpha-topic",
            ),
        ];
    }
    Vec::new()
}

#[tokio::test]
async fn a_malformed_resource_scan_id_is_an_input_constraint_violation() {
    let router = router();

    for IdCase {
        case,
        resource_scan_id,
        well_formed,
    } in id_cases()
    {
        for action in [
            "DescribeResourceScan",
            "ListResourceScanResources",
            "ListResourceScanRelatedResources",
        ] {
            let mut members = vec![member("ResourceScanId", &resource_scan_id)];
            members.extend(beside_the_id(action));
            let (status, body) = call(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected 400, got {status} {body}"
            );
            let code = error_code(case, &body);
            let message = xml_text(&body, "Message").unwrap_or_default();
            if well_formed {
                // The value addresses a scan that COULD exist, so the read got as far as the
                // lookup: the refusal is the not-found one, with its own fixed sentence.
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("ResourceScanNotFound", scans::NOT_FOUND),
                    "case {case:?} on {action}: expected the lookup failure, got {body}"
                );
                continue;
            }
            assert_eq!(
                code, "ValidationError",
                "case {case:?} on {action}: expected the input-constraint refusal, got {body}"
            );
            assert_eq!(
                message,
                expected_pattern_refusal(&resource_scan_id),
                "case {case:?} on {action}: expected the pattern refusal, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. A double is written as a double
// ---------------------------------------------------------------------------

/// Whether `rendered` is a floating-point literal written the way a double is: it parses as
/// one, and it carries a fractional part rather than reading as an integer.
fn is_written_as_a_double(rendered: &str) -> bool {
    rendered.parse::<f64>().is_ok() && rendered.contains('.')
}

/// Every `PercentageCompleted` one response carries, in document order.
fn percentages(body: &str) -> Vec<String> {
    xml_texts(body, "PercentageCompleted")
}

#[tokio::test]
async fn percentage_completed_is_written_as_a_double() {
    let router = router();
    deploy(&router, "read-shapes-doubles", TOPIC_AND_BUCKET).await;
    let resource_scan_id = start_a_scan(&router).await;

    // Both readings a scan's life passes through — the scan still running, and the scan
    // concluded — from the item read and from the listing, which report the same member off
    // the same record.
    let mut readings: Vec<(String, String)> = Vec::new();
    for stage in ["while the scan is running", "once the scan has concluded"] {
        for (surface, members) in [
            (
                "DescribeResourceScan",
                vec![member("ResourceScanId", &resource_scan_id)],
            ),
            ("ListResourceScans", Vec::new()),
        ] {
            let (status, body) = call(&router, surface, &members).await;
            assert_eq!(
                status, 200,
                "case {stage:?} on {surface}: expected 200, got {status} {body}"
            );
            let reported = percentages(&body);
            assert!(
                !reported.is_empty(),
                "case {stage:?} on {surface}: expected a PercentageCompleted, got {body}"
            );
            for rendered in reported {
                readings.push((format!("{stage} on {surface}"), rendered));
            }
        }
        if stage == "while the scan is running" {
            wait_for_the_scan().await;
        }
    }

    for (case, rendered) in &readings {
        assert!(
            is_written_as_a_double(rendered),
            "case {case:?}: expected a double literal with a fractional part, got {rendered:?}"
        );
    }
    // And the concluded reading is the whole hundred per cent, written as the double it is.
    let concluded: Vec<&String> = readings
        .iter()
        .filter(|(case, _)| case.starts_with("once the scan has concluded"))
        .map(|(_, rendered)| rendered)
        .collect();
    assert!(
        concluded
            .iter()
            .all(|rendered| rendered.as_str() == "100.0"),
        "case \"a concluded scan\": expected every reading to be 100.0, got {concluded:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. ResourceTypes is gated by the status, not by being empty
// ---------------------------------------------------------------------------

/// One row of the resource-types table: what the account holds when the scan runs, and the
/// distinct resource types a completed scan of it must therefore report.
struct TypesCase {
    case: &'static str,
    template: Option<&'static str>,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn a_completed_scan_reports_the_resource_types_its_status_admits() {
    for TypesCase {
        case,
        template,
        expected,
    } in [
        TypesCase {
            case: "an account with resources of two types",
            template: Some(TOPIC_AND_BUCKET),
            expected: vec!["AWS::S3::Bucket", "AWS::SNS::Topic"],
        },
        TypesCase {
            case: "an account with nothing to find",
            template: None,
            expected: Vec::new(),
        },
    ] {
        let router = router();
        if let Some(template) = template {
            deploy(&router, "read-shapes-types", template).await;
        }
        let resource_scan_id = start_a_scan(&router).await;

        // While the scan runs its types are not available, so the member is absent rather
        // than empty: the STATUS is what gates it.
        let (status, body) = call(
            &router,
            "DescribeResourceScan",
            &[member("ResourceScanId", &resource_scan_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 while the scan runs, got {status} {body}"
        );
        assert!(
            xml_blocks(&body, "ResourceTypes").is_empty(),
            "case {case:?}: expected no ResourceTypes while the scan runs, got {body}"
        );

        wait_for_the_scan().await;
        let (status, body) = call(
            &router,
            "DescribeResourceScan",
            &[member("ResourceScanId", &resource_scan_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 once the scan concluded, got {status} {body}"
        );
        let block = xml_blocks(&body, "ResourceTypes");
        assert_eq!(
            block.len(),
            1,
            "case {case:?}: expected exactly one ResourceTypes element, got {body}"
        );
        let mut reported = xml_texts(&block[0], "member");
        reported.sort();
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the scan's distinct resource types, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. The order an enum's value set is stated in
// ---------------------------------------------------------------------------

/// One row of the enum-order table: the member a bad value is sent in, and the value set the
/// refusal must list — in the SERVICE's order where it has one, and in the model's
/// declaration order otherwise.
struct EnumCase {
    case: &'static str,
    action: &'static str,
    request_member: &'static str,
    path: &'static str,
    value_set: &'static str,
}

#[tokio::test]
async fn an_enum_value_set_is_stated_in_the_order_the_service_lists_it() {
    let router = router();

    for EnumCase {
        case,
        action,
        request_member,
        path,
        value_set,
    } in [
        EnumCase {
            case: "a registration status",
            action: "ListTypeRegistrations",
            request_member: "RegistrationStatusFilter",
            path: "registrationStatusFilter",
            // Source: CloudFormation's own refusal lists `RegistrationStatus` in the order a
            // registration passes through, which the model does not declare it in.
            value_set: "[IN_PROGRESS, COMPLETE, FAILED]",
        },
        EnumCase {
            case: "a scan type, which the service lists as the model declares it",
            action: "ListResourceScans",
            request_member: "ScanTypeFilter",
            path: "scanTypeFilter",
            // Source: `smithy_query(query="shape", shape="ScanType")` — `FULL | PARTIAL`.
            value_set: "[FULL, PARTIAL]",
        },
        EnumCase {
            case: "a visibility, which the service lists as the model declares it",
            action: "ListTypes",
            request_member: "Visibility",
            path: "visibility",
            // Source: `smithy_query(query="shape", shape="Visibility")` — `PUBLIC | PRIVATE`.
            value_set: "[PUBLIC, PRIVATE]",
        },
    ] {
        // Several values outside the set, so the sentence is not fitted to one of them.
        for supplied in ["NOT_A_REAL_STATUS", "complete", "", "0"] {
            let (status, body) =
                call(&router, action, &[member(request_member, supplied)]).await;
            assert_eq!(
                status, 400,
                "case {case:?} with {supplied:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(case, &body),
                "ValidationError",
                "case {case:?} with {supplied:?}: expected the input refusal, got {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "1 validation error detected: Value '{supplied}' at '{path}' failed to \
                     satisfy constraint: Member must satisfy enum value set: {value_set}"
                ),
                "case {case:?} with {supplied:?}: expected the service's own value set \
                 order, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 5. What the registry reads report
// ---------------------------------------------------------------------------

/// The repository AWS publishes a resource type's provider in, as its CLONE url.
///
/// Source: AWS's CloudFormation resource providers are open source, one repository per
/// service named after the type's own service segment —
/// `aws-cloudformation-resource-providers-<service>` — and a clone url ends in `.git`.
/// Derived here the same way the registry derives it, so a type the issue never cited is
/// covered too.
fn expected_clone_url(type_name: &str) -> String {
    let service = type_name.split("::").nth(1).unwrap_or_default();
    format!(
        "https://github.com/aws-cloudformation/aws-cloudformation-resource-providers-{}.git",
        service.to_ascii_lowercase()
    )
}

#[tokio::test]
async fn a_published_extension_reports_the_clone_url_of_its_provider_repository() {
    let router = router();

    // Types spanning the services they belong to, including ones the issue never cited.
    for type_name in [
        "AWS::IAM::Role",
        "AWS::S3::Bucket",
        "AWS::Scheduler::Schedule",
        "AWS::ElasticLoadBalancingV2::TargetGroup",
        "AWS::BedrockAgentCore::Runtime",
    ] {
        let (status, body) = call(
            &router,
            "DescribeType",
            &[member("Type", "RESOURCE"), member("TypeName", type_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {type_name:?}: expected DescribeType to answer, got {status} {body}"
        );
        let source_url = xml_text(&body, "SourceUrl");
        assert_eq!(
            source_url.as_deref(),
            Some(expected_clone_url(type_name).as_str()),
            "case {type_name:?}: expected the provider repository's clone url, got \
             {source_url:?}"
        );
    }
}

/// One row of the batch table: how an identifier the registry holds no configuration for
/// names its extension, and how many identifiers the request carries.
struct BatchCase {
    case: &'static str,
    identifiers: usize,
    members: Vec<(String, String)>,
}

#[tokio::test]
async fn the_configuration_batch_answers_all_three_of_its_result_lists() {
    let router = router();

    for BatchCase {
        case,
        identifiers,
        members,
    } in [
        BatchCase {
            case: "an extension named by kind and name",
            identifiers: 1,
            members: vec![
                member("TypeConfigurationIdentifiers.member.1.Type", "RESOURCE"),
                member(
                    "TypeConfigurationIdentifiers.member.1.TypeName",
                    "AWS::IAM::Role",
                ),
            ],
        },
        BatchCase {
            case: "an extension named by its ARN",
            identifiers: 1,
            members: vec![member(
                "TypeConfigurationIdentifiers.member.1.TypeArn",
                &format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/hook/No-Such-Hook"),
            )],
        },
        BatchCase {
            case: "a configuration named by its own ARN",
            identifiers: 1,
            members: vec![member(
                "TypeConfigurationIdentifiers.member.1.TypeConfigurationArn",
                &format!(
                    "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type-configuration/resource/\
                     No-Such-Resource/default"
                ),
            )],
        },
        BatchCase {
            case: "two identifiers at once",
            identifiers: 2,
            members: vec![
                member(
                    "TypeConfigurationIdentifiers.member.1.TypeArn",
                    &format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:type/module/No-Module"),
                ),
                member("TypeConfigurationIdentifiers.member.2.Type", "HOOK"),
                member(
                    "TypeConfigurationIdentifiers.member.2.TypeName",
                    "My::Sample::Hook",
                ),
            ],
        },
    ] {
        let (status, body) = call(&router, "BatchDescribeTypeConfigurations", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch to succeed, got {status} {body}"
        );
        // All three result members are present, whether or not this batch filled them.
        for list in [
            "Errors",
            "UnprocessedTypeConfigurations",
            "TypeConfigurations",
        ] {
            assert_eq!(
                xml_blocks(&body, list).len(),
                1,
                "case {case:?}: expected exactly one <{list}> element, got {body}"
            );
        }
        let errors = xml_blocks(&body, "Errors")
            .into_iter()
            .next()
            .unwrap_or_default();
        let entries = xml_blocks(&errors, "member");
        assert_eq!(
            entries.len(),
            identifiers,
            "case {case:?}: expected one error entry per identifier, got {body}"
        );
        for entry in &entries {
            assert_eq!(
                (
                    xml_text(entry, "ErrorCode").unwrap_or_default().as_str(),
                    xml_text(entry, "ErrorMessage").unwrap_or_default().as_str(),
                ),
                ("404", "TypeConfiguration not found"),
                "case {case:?}: expected the registry's own wording for a configuration it \
                 does not hold, got {entry:?}"
            );
        }
        // And nothing was resolved, so the other two lists are the empty elements.
        assert!(
            xml_blocks(&body, "TypeConfigurations")
                .first()
                .is_some_and(String::is_empty),
            "case {case:?}: expected an empty TypeConfigurations element, got {body}"
        );
    }
}
