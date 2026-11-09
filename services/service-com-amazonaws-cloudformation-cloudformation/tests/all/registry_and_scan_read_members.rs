// SPDX-License-Identifier: CC0-1.0

//! What the registry and resource-scan READS put on the wire, member by member — end to end
//! over the awsQuery wire. (`scan_and_registry_reads.rs` is the sibling suite for the refusal
//! shapes and the double rendering of the same two surfaces.)
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **A scan reports the per-resource-type BREAKDOWN of what it listed.**
//!    `ResourceTypeSummaries` is present on every read of a scan — empty while the scan has no
//!    per-type result to report — and once the scan's results are readable it carries one
//!    summary per distinct resource type among the rows the scan listed, with how many of them
//!    that type accounts for. It is computed from the same rows `ResourceTypes` takes the
//!    distinct types of and `ResourcesScanned` totals, so the three cannot disagree: the table
//!    asserts the breakdown against both of them on every row.
//!
//! 2. **A summary of a PUBLISHED public extension carries its publisher, its description and
//!    its public version.** The registry of every account and Region holds the public
//!    extensions Amazon Web Services publishes, and those are not of one KIND: the resource
//!    types it provides and the Hooks it has published are both in it
//!    (`state::registry::PUBLIC_EXTENSION_CATALOG`). A record a publisher published reports
//!    the publisher's id, name and verified identity, the description the catalogue carries
//!    and the public version the registry holds it at, whether or not this account activated
//!    it; one with no publisher behind it reports none of those. The table is one row per
//!    catalogued extension, read through the listing, narrowed by each filter that must admit
//!    or exclude it, and read back through `DescribeType` at the ARN the listing reported.
//!
//! 3. **A listing emits a `NextToken` whenever records remain past the page it returned.**
//!    `@paginated(inputToken: NextToken, outputToken: NextToken, items: TypeSummaries,
//!    pageSize: MaxResults)`: a page carries at most the effective page size — the request's
//!    `MaxResults`, or this service's own where it names none — and the token resumes
//!    immediately past it, so the pages of one listing partition it exactly once and the last
//!    page carries no token. The table spans a page size the caller chose, the one it did not,
//!    and a listing that fits in one page, over a registry holding more records than the
//!    default page size.
//!
//! The registry's WRITERS (`RegisterType`, `ActivateType`) are scaffold stubs with their own
//! issues, so the records a case needs beyond the published catalogue are seeded straight into
//! the store — the same device `extension_registry.rs` uses. Every READ goes through the real
//! request path (form-encoded body in, XML envelope out), so what is asserted is what a client
//! sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::publisher::identity_provider;
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    published_catalog_arn, published_publisher_id, registry_type, visibility, StoredExtension,
    StoredTypeVersion, FIRST_PUBLIC_VERSION_NUMBER, PUBLIC_EXTENSION_CATALOG,
};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use service_com_amazonaws_cloudformation_cloudformation::state::PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{member, router, router_with_engine, store, xml_blocks, xml_text, xml_texts};
use crate::stack_lifecycle::{call, deploy, REGION};

// ---------------------------------------------------------------------------
// 1. The per-resource-type breakdown of a scan
// ---------------------------------------------------------------------------

/// A template declaring one resource.
const ONE_TOPIC: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A template declaring two resources of two types, so a row spans more than one type.
const TOPIC_AND_BUCKET: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// Wait past the instant a started scan's conclusion becomes due.
async fn wait_for_the_scan() {
    tokio::time::sleep(scans::SCAN_DELAY + Duration::from_millis(250)).await;
}

/// `DescribeResourceScan` for one scan.
async fn describe_resource_scan(router: &MultiServiceRouter, resource_scan_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeResourceScan",
        &[member("ResourceScanId", resource_scan_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a read of a scan\": expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The one `<ResourceTypeSummaries>` element of a response, which every read of a scan must
/// carry exactly one of.
fn breakdown_element(case: &str, body: &str) -> String {
    let blocks = xml_blocks(body, "ResourceTypeSummaries");
    assert_eq!(
        blocks.len(),
        1,
        "case {case:?}: expected exactly one ResourceTypeSummaries element, got {body}"
    );
    blocks.into_iter().next().expect("checked just above")
}

/// The breakdown a response carries, as (resource type, count) pairs in document order.
fn breakdown(case: &str, body: &str) -> Vec<(String, usize)> {
    let element = breakdown_element(case, body);
    xml_blocks(&element, "member")
        .iter()
        .map(|summary| {
            let resource_type = xml_text(summary, "ResourceType").unwrap_or_else(|| {
                panic!("case {case:?}: expected a ResourceType in {summary:?}")
            });
            let scanned = xml_text(summary, "ResourcesScanned")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ResourcesScanned in {summary:?}")
                })
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("case {case:?}: expected a count in {summary:?}"));
            (resource_type, scanned)
        })
        .collect()
}

/// One row of the breakdown table: what the account holds when the scan runs, the filter the
/// call supplies, and the per-type breakdown the concluded scan must report.
struct BreakdownCase {
    case: &'static str,
    templates: &'static [&'static str],
    filter: &'static [&'static str],
    expected: &'static [(&'static str, usize)],
}

/// The rows: an account holding nothing, one holding one resource, one holding several of two
/// types across two stacks (so a type's count is above one), and a filtered scan, whose
/// breakdown counts what the scan LISTED rather than what it read.
const BREAKDOWN_CASES: &[BreakdownCase] = &[
    BreakdownCase {
        case: "an account holding no resources",
        templates: &[],
        filter: &[],
        expected: &[],
    },
    BreakdownCase {
        case: "one resource of one type",
        templates: &[ONE_TOPIC],
        filter: &[],
        expected: &[("AWS::SNS::Topic", 1)],
    },
    BreakdownCase {
        case: "three resources of two types across two stacks",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &[],
        expected: &[("AWS::S3::Bucket", 1), ("AWS::SNS::Topic", 2)],
    },
    BreakdownCase {
        case: "a filter that leaves one type listed",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &["AWS::S3::*"],
        expected: &[("AWS::S3::Bucket", 1)],
    },
    BreakdownCase {
        case: "a filter naming a type the account holds none of",
        templates: &[TOPIC_AND_BUCKET],
        filter: &["AWS::DynamoDB::Table"],
        expected: &[],
    },
];

/// The one `ScanFilters` entry naming `types`, in the awsQuery flattened form.
fn filter_members(types: &[&str]) -> Vec<(String, String)> {
    types
        .iter()
        .enumerate()
        .map(|(position, value)| {
            member(
                &format!("ScanFilters.member.1.Types.member.{}", position + 1),
                value,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_scan_reports_the_per_type_breakdown_of_what_it_listed() {
    for (index, row) in BREAKDOWN_CASES.iter().enumerate() {
        let case = row.case;
        // One router per row: what the account holds is what the row is about.
        let router = router();
        for (position, template) in row.templates.iter().enumerate() {
            deploy(&router, &format!("breakdown-{index}-{position}"), template).await;
        }

        let (status, body) =
            call(&router, "StartResourceScan", &filter_members(row.filter)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartResourceScan answered {status} with {body}"
        );
        let resource_scan_id = xml_text(&body, "ResourceScanId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"));

        // While the scan runs its per-type results are not readable — but the element is
        // there, empty, so a client iterating the breakdown sees the same shape throughout.
        let body = describe_resource_scan(&router, &resource_scan_id).await;
        let running = breakdown(case, &body);
        assert!(
            running.is_empty(),
            "case {case:?}: expected an empty breakdown while the scan runs, got {running:?} \
             from {body}"
        );

        wait_for_the_scan().await;
        let body = describe_resource_scan(&router, &resource_scan_id).await;
        let reported = breakdown(case, &body);
        let expected: Vec<(String, usize)> = row
            .expected
            .iter()
            .map(|(resource_type, scanned)| ((*resource_type).to_string(), *scanned))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the per-type breakdown {expected:?}, got {reported:?} \
             from {body}"
        );

        // The breakdown is the same set of rows the other two members report, so it agrees
        // with both: its types are `ResourceTypes`, and its counts sum to `ResourcesScanned`.
        let types: Vec<String> = reported
            .iter()
            .map(|(resource_type, _)| resource_type.clone())
            .collect();
        let mut listed_types = xml_texts(
            &xml_blocks(&body, "ResourceTypes")
                .into_iter()
                .next()
                .unwrap_or_default(),
            "member",
        );
        listed_types.sort();
        assert_eq!(
            types, listed_types,
            "case {case:?}: expected the breakdown to cover the scan's resource types \
             {listed_types:?}, got {types:?} from {body}"
        );
        let total: usize = reported.iter().map(|(_, scanned)| scanned).sum();
        let resources_scanned = xml_text(&body, "ResourcesScanned")
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResourcesScanned, got {body}"))
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("case {case:?}: expected a count, got {body}"));
        assert_eq!(
            total, resources_scanned,
            "case {case:?}: expected the breakdown to total the resources the scan listed \
             ({resources_scanned}), got {total} from {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. The handler permissions an extension's schema declares
// ---------------------------------------------------------------------------

/// An instant to register a seeded version at, so nothing a case asserts comes from the clock.
fn registered_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + offset_secs)
}

/// A private extension record holding one version with `schema` as its schema.
fn private_record_with_schema(
    type_name: &str,
    schema: Option<&str>,
    offset_secs: u64,
) -> StoredExtension {
    let mut extension = StoredExtension::new(REGION, registry_type::RESOURCE, type_name);
    let mut version =
        StoredTypeVersion::new(&extension.arn, "00000001", registered_at(offset_secs));
    version.schema = schema.map(str::to_string);
    extension.default_version_id = Some("00000001".to_string());
    extension.versions = vec![version];
    extension
}

/// A resource provider schema declaring handlers, each with the permissions its handler needs
/// — the `handlers` section of the provider definition schema a registration submits.
const SCHEMA_WITH_HANDLERS: &str = r#"{
    "typeName":"Seeded::Handlers::Resource",
    "handlers":{
        "create":{"permissions":["s3:CreateBucket","s3:PutBucketTagging"]},
        "read":{"permissions":["s3:GetBucketTagging"]},
        "delete":{"permissions":["s3:DeleteBucket","s3:CreateBucket"]}
    }
}"#;

/// The same document with a handler that needs no permission at all.
const SCHEMA_WITH_AN_EMPTY_HANDLER: &str = r#"{
    "typeName":"Seeded::Empty::Resource",
    "handlers":{"read":{"permissions":[]}}
}"#;

/// A schema with no `handlers` section, which is what the registry derives for the resource
/// types it publishes.
const SCHEMA_WITHOUT_HANDLERS: &str = r#"{"typeName":"Seeded::Plain::Resource"}"#;

/// One row of the handler-permissions table: how the request names the extension, and the
/// permissions the response must report.
struct PermissionsCase {
    case: &'static str,
    members: Vec<(String, String)>,
    expected: Vec<&'static str>,
}

/// A router over a registry seeded with one record per kind of schema a version can hold.
fn registry_with_schemas() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        state.lock().extensions = vec![
            private_record_with_schema(
                "Seeded::Handlers::Resource",
                Some(SCHEMA_WITH_HANDLERS),
                0,
            ),
            private_record_with_schema(
                "Seeded::Empty::Resource",
                Some(SCHEMA_WITH_AN_EMPTY_HANDLER),
                60,
            ),
            private_record_with_schema(
                "Seeded::Plain::Resource",
                Some(SCHEMA_WITHOUT_HANDLERS),
                120,
            ),
            private_record_with_schema("Seeded::NoSchema::Resource", None, 180),
            private_record_with_schema("Seeded::Broken::Resource", Some("not a schema"), 240),
        ];
    }
    (router, engine)
}

/// `DescribeType` reports the permissions the handlers declared in the described version's
/// schema require — the union of them, as a set, and an EMPTY set rather than no element at
/// all for every record whose schema declares none.
#[tokio::test]
async fn describe_type_reports_the_handler_permissions_its_schema_declares() {
    let by_name = |type_kind: &str, type_name: &str| {
        vec![
            member("Type", type_kind),
            member("TypeName", type_name),
        ]
    };
    let mut cases = vec![
        PermissionsCase {
            case: "a schema declaring handlers, each with its permissions",
            members: by_name(registry_type::RESOURCE, "Seeded::Handlers::Resource"),
            // The union of every handler's list, deduplicated and sorted: one permission is
            // declared by two handlers, and the set is what the member reports.
            expected: vec![
                "s3:CreateBucket",
                "s3:DeleteBucket",
                "s3:GetBucketTagging",
                "s3:PutBucketTagging",
            ],
        },
        PermissionsCase {
            case: "a schema whose handler needs no permission",
            members: by_name(registry_type::RESOURCE, "Seeded::Empty::Resource"),
            expected: Vec::new(),
        },
        PermissionsCase {
            case: "a schema with no handlers section",
            members: by_name(registry_type::RESOURCE, "Seeded::Plain::Resource"),
            expected: Vec::new(),
        },
        PermissionsCase {
            case: "a version the registry holds no schema for",
            members: by_name(registry_type::RESOURCE, "Seeded::NoSchema::Resource"),
            expected: Vec::new(),
        },
        PermissionsCase {
            case: "a schema that is no JSON document at all",
            members: by_name(registry_type::RESOURCE, "Seeded::Broken::Resource"),
            expected: Vec::new(),
        },
    ];
    // And the records nobody seeded: a resource type Amazon Web Services provides, whose
    // derived schema declares no handler, and a Hook it has published, which the registry
    // holds no schema for at all.
    cases.push(PermissionsCase {
        case: "a resource type the registry publishes",
        members: by_name(registry_type::RESOURCE, "AWS::S3::Bucket"),
        expected: Vec::new(),
    });
    for published in PUBLIC_EXTENSION_CATALOG {
        cases.push(PermissionsCase {
            case: "a published extension of the catalogue",
            members: by_name(published.type_kind, published.type_name),
            expected: Vec::new(),
        });
    }

    let (router, _engine) = registry_with_schemas();
    for PermissionsCase {
        case,
        members,
        expected,
    } in cases
    {
        let named = members
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<String>>()
            .join(" ");
        let (status, body) = call(&router, "DescribeType", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({named}): expected HTTP 200, got {status} with {body}"
        );
        let blocks = xml_blocks(&body, "SchemaHandlerPermissions");
        assert_eq!(
            blocks.len(),
            1,
            "case {case:?} ({named}): expected exactly one SchemaHandlerPermissions \
             element, got {body}"
        );
        let reported = xml_texts(&blocks[0], "member");
        let expected: Vec<String> = expected.iter().map(|one| (*one).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?} ({named}): expected the handler permissions {expected:?}, got \
             {reported:?} from {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The published catalogue: every kind, with its publisher metadata
// ---------------------------------------------------------------------------

async fn list_types(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call(router, "ListTypes", &members).await
}

/// Every `<member>` summary of a listing, in the order the response carries them.
fn summaries(body: &str) -> Vec<String> {
    let Some(element) = xml_blocks(body, "TypeSummaries").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&element, "member")
}

/// The `TypeName` of every summary of a listing, in the order the response carries them.
fn listed_names(body: &str) -> Vec<String> {
    summaries(body)
        .iter()
        .filter_map(|summary| xml_text(summary, "TypeName"))
        .collect()
}

/// Each extension the registry PUBLISHES is listed in the public registry with the publisher,
/// the description and the public version its record carries, is narrowed by each filter that
/// must admit or exclude it, and is describable at the ARN the listing reported.
#[tokio::test]
async fn the_published_catalogue_reports_its_publisher_and_version_metadata() {
    let router = router();

    // The catalogue is not of one KIND: the rule is about the registry holding the published
    // extensions of every kind, so the table must span more than resource types.
    let kinds: Vec<&str> = PUBLIC_EXTENSION_CATALOG
        .iter()
        .map(|published| published.type_kind)
        .collect();
    assert!(
        kinds.iter().any(|kind| *kind != registry_type::RESOURCE),
        "case \"the published catalogue\": expected a published extension of a kind other \
         than RESOURCE, got {kinds:?}"
    );

    for published in PUBLIC_EXTENSION_CATALOG {
        let case = published.type_name;
        let (status, body) = list_types(
            &router,
            &[
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", published.type_name),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let listed = summaries(&body);
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one summary, got {listed:?} from {body}"
        );
        let summary = &listed[0];
        let publisher_id = published_publisher_id(published.publisher_name);
        // The ARN the registry addresses a catalogued extension by: the one that names its
        // publisher where a third party published it, and the one that names none where
        // Amazon Web Services did.
        let type_arn = published_catalog_arn(REGION, published);
        for (tag, expected) in [
            ("Type", published.type_kind.to_string()),
            ("TypeName", published.type_name.to_string()),
            ("TypeArn", type_arn.clone()),
            ("Description", published.description.to_string()),
            ("PublisherId", publisher_id.clone()),
            ("PublisherName", published.publisher_name.to_string()),
            (
                "PublisherIdentity",
                identity_provider::AWS_MARKETPLACE.to_string(),
            ),
            (
                "LatestPublicVersion",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
            // "Whether the extension is activated for this account and Region": nothing
            // activated it here, and that is reported rather than left out.
            ("IsActivated", "false".to_string()),
        ] {
            let actual = xml_text(summary, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{tag}> to be {expected:?}, got {actual:?} from \
                 {summary:?}"
            );
        }
        assert!(
            xml_text(summary, "LastUpdated").is_some(),
            "case {case:?}: expected the instant the published version entered the registry, \
             got {summary:?}"
        );
        // A publisher id is an opaque identifier of the publisher, in the form the member
        // models (`@pattern("^[0-9a-zA-Z]{12,40}$")`), and the same one on every read.
        assert!(
            publisher_id.len() >= 12
                && publisher_id.len() <= 40
                && publisher_id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected a modelled PublisherId, got {publisher_id:?}"
        );

        // Narrowed by its own kind it is there; narrowed by another kind, or by a publisher
        // that published nothing here, it is not.
        let (_, narrowed) = list_types(
            &router,
            &[
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", published.type_name),
                ("Type", published.type_kind),
            ],
        )
        .await;
        assert_eq!(
            listed_names(&narrowed),
            vec![published.type_name.to_string()],
            "case {case:?}: expected the record under its own kind, got {narrowed}"
        );
        let (_, by_publisher) = list_types(
            &router,
            &[
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", published.type_name),
                ("Filters.PublisherId", &publisher_id),
            ],
        )
        .await;
        assert_eq!(
            listed_names(&by_publisher),
            vec![published.type_name.to_string()],
            "case {case:?}: expected the record under its own publisher, got {by_publisher}"
        );
        let (_, private_listing) = list_types(
            &router,
            &[("Filters.TypeNamePrefix", published.type_name)],
        )
        .await;
        assert!(
            listed_names(&private_listing).is_empty(),
            "case {case:?}: expected a PUBLIC record to be outside the private listing, got \
             {private_listing}"
        );

        // The listing and the detailed read are two views of ONE registry: the record is
        // describable at the ARN the summary reported, and reports the same publisher.
        let (status, described) = call(
            &router,
            "DescribeType",
            &[member("Arn", &type_arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeType of {type_arn:?} to answer HTTP 200, got \
             {status} with {described}"
        );
        for (tag, expected) in [
            ("TypeName", published.type_name.to_string()),
            ("Type", published.type_kind.to_string()),
            ("Visibility", visibility::PUBLIC.to_string()),
            ("PublisherId", publisher_id.clone()),
            (
                "LatestPublicVersion",
                FIRST_PUBLIC_VERSION_NUMBER.to_string(),
            ),
        ] {
            let actual = xml_text(&described, tag);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected DescribeType's <{tag}> to be {expected:?}, got \
                 {actual:?} from {described}"
            );
        }
    }
}

/// A public record with NO publisher behind it reports none of the publisher members, and no
/// latest published version: the rule is about the record's publisher, not about its
/// visibility.
#[tokio::test]
async fn a_public_record_nobody_published_reports_no_publisher_metadata() {
    let router = router();
    let provided = "AWS::S3::Bucket";

    let (status, body) = list_types(
        &router,
        &[
            ("Visibility", visibility::PUBLIC),
            ("Filters.TypeNamePrefix", provided),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {provided:?}: expected HTTP 200, got {status} with {body}"
    );
    let listed = summaries(&body);
    assert_eq!(
        listed.len(),
        1,
        "case {provided:?}: expected exactly one summary, got {listed:?} from {body}"
    );
    for tag in [
        "PublisherId",
        "PublisherName",
        "PublisherIdentity",
        "LatestPublicVersion",
        "IsActivated",
    ] {
        let actual = xml_text(&listed[0], tag);
        assert_eq!(
            actual, None,
            "case {provided:?}: expected no <{tag}> on a record nobody published, got \
             {actual:?} from {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. NextToken whenever records remain past the page
// ---------------------------------------------------------------------------

/// How many records the paging fixture seeds: more than the page size a request that names no
/// `MaxResults` gets, so the FIRST page of such a listing leaves records behind.
const SEEDED_RECORDS: usize = PAGE_SIZE + 3;

/// The name of the `index`th seeded record, in the form the modelled `TypeName` states.
fn paged_type_name(index: usize) -> String {
    format!("Paged::Sample::Extension{index:04}")
}

/// A router over a registry holding [`SEEDED_RECORDS`] private records, spanning every
/// extension kind so the walk is not of one kind either.
fn registry_with_many_records() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    let kinds = [
        registry_type::RESOURCE,
        registry_type::MODULE,
        registry_type::HOOK,
    ];
    let records: Vec<StoredExtension> = (0..SEEDED_RECORDS)
        .map(|index| {
            let type_kind = kinds[index % kinds.len()];
            let type_name = paged_type_name(index);
            // A module's name ends in `::MODULE`, which the modelled `TypeName` pattern
            // admits as a fourth segment.
            let type_name = if type_kind == registry_type::MODULE {
                format!("{type_name}::MODULE")
            } else {
                type_name
            };
            let mut extension = StoredExtension::new(REGION, type_kind, &type_name);
            let version = StoredTypeVersion::new(
                &extension.arn,
                "00000001",
                registered_at(1_000 + index as u64),
            );
            extension.default_version_id = Some("00000001".to_string());
            extension.versions = vec![version];
            extension
        })
        .collect();
    store(&engine).lock().extensions = records;
    (router, engine)
}

/// A listing carries a `NextToken` exactly while records remain past the page it returned, and
/// the pages it hands back partition the listing once — whatever page size the request chose,
/// including none at all.
#[tokio::test]
async fn a_listing_emits_a_next_token_while_records_remain() {
    let (router, _engine) = registry_with_many_records();

    // Every page size a request can take: none (the service's own), the modelled minimum, a
    // size that does not divide the listing, and the modelled maximum.
    for max_results in [None, Some(1usize), Some(7), Some(100)] {
        let case = format!("MaxResults={max_results:?}");
        let page_size = max_results.unwrap_or(PAGE_SIZE).min(PAGE_SIZE);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let max = max_results.map(|max| max.to_string());
            let mut members: Vec<(&str, &str)> = Vec::new();
            if let Some(max) = max.as_deref() {
                members.push(("MaxResults", max));
            }
            if let Some(token) = token.as_deref() {
                members.push(("NextToken", token));
            }
            let (status, body) = list_types(&router, &members).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 on page {pages}, got {status} with {body}"
            );
            let page = listed_names(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} summaries on page {pages}, got \
                 {page:?}"
            );
            walked.extend(page.clone());
            token = xml_text(&body, "NextToken");
            pages += 1;
            let remaining = SEEDED_RECORDS - walked.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case:?}: expected a NextToken on page {pages} exactly while records \
                 remain ({remaining} left), got {token:?} from {body}"
            );
            if token.is_none() {
                break;
            }
            assert!(
                pages <= SEEDED_RECORDS,
                "case {case:?}: the listing did not end within {SEEDED_RECORDS} pages"
            );
        }
        let mut expected: Vec<String> = (0..SEEDED_RECORDS)
            .map(|index| {
                let name = paged_type_name(index);
                if registry_type::MODULE
                    == [
                        registry_type::RESOURCE,
                        registry_type::MODULE,
                        registry_type::HOOK,
                    ][index % 3]
                {
                    format!("{name}::MODULE")
                } else {
                    name
                }
            })
            .collect();
        expected.sort();
        let mut seen = walked.clone();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the pages to partition the listing exactly once, got \
             {walked:?}"
        );
        assert_eq!(
            pages,
            SEEDED_RECORDS.div_ceil(page_size),
            "case {case:?}: expected {} pages over {SEEDED_RECORDS} records, got {pages}",
            SEEDED_RECORDS.div_ceil(page_size)
        );
    }
}

/// A listing that FITS in the page it returned carries no token at all: "if the request
/// returns all results, NextToken is set to null".
#[tokio::test]
async fn a_listing_that_fits_in_one_page_carries_no_token() {
    let (router, _engine) = registry_with_many_records();

    for (case, members) in [
        (
            "a narrowed listing of one record",
            vec![
                ("Filters.TypeNamePrefix", paged_type_name(0).as_str()),
            ],
        ),
        ("a listing narrowed to nothing", vec![
            ("Filters.TypeNamePrefix", "Nothing::Under::This"),
        ]),
    ] {
        let members: Vec<(&str, &str)> = members;
        let (status, body) = list_types(&router, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: expected no NextToken when the page carries the whole listing, \
             got {body}"
        );
    }
}
